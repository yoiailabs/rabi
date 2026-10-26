#[path = "common/records.rs"]
mod records;
#[path = "common/git.rs"]
mod test_git;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Repo {
    root: PathBuf,
    main: PathBuf,
    upstream: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-sync-{}", uuid::Uuid::new_v4()));
        let main = root.join("main");
        let upstream = root.join("upstream");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        test_git::write_config(&root.join("home"));
        let repo = Self {
            root,
            main,
            upstream,
        };
        repo.git(&repo.main, &["init", "-b", "main"]);
        repo.git(&repo.main, &["config", "user.name", "Sync Test"]);
        repo.git(&repo.main, &["config", "user.email", "sync@example.com"]);
        repo.write_commit(&repo.main, "file", "initial\n");
        repo.git(
            &repo.main,
            &[
                "clone",
                "--bare",
                repo.main.to_str().unwrap(),
                repo.root.join("remote.git").to_str().unwrap(),
            ],
        );
        repo.git(
            &repo.main,
            &[
                "remote",
                "add",
                "origin",
                repo.root.join("remote.git").to_str().unwrap(),
            ],
        );
        repo.git(&repo.main, &["fetch", "origin"]);
        repo.git(&repo.main, &["remote", "set-head", "origin", "main"]);
        repo.git(
            &repo.main,
            &[
                "clone",
                repo.root.join("remote.git").to_str().unwrap(),
                repo.upstream.to_str().unwrap(),
            ],
        );
        repo.git(&repo.upstream, &["config", "user.name", "Sync Test"]);
        repo.git(
            &repo.upstream,
            &["config", "user.email", "sync@example.com"],
        );
        repo
    }
    fn command(&self, program: &str, path: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(path)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("home/.gitconfig"))
            .env("GIT_TEMPLATE_DIR", self.root.join("template"))
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        fs::create_dir_all(self.root.join("template")).unwrap();
        cmd
    }
    fn git(&self, path: &Path, args: &[&str]) -> String {
        let out = self.command("git", path, args).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
    fn keep_remote_head_absent(&self, remote: &str) {
        // Git >= 2.48 otherwise creates this symref again on fetch. Older Git
        // ignores the setting, so both versions exercise the headless fallback.
        self.git(
            &self.main,
            &[
                "config",
                &format!("remote.{remote}.followRemoteHEAD"),
                "never",
            ],
        );
        self.git(&self.main, &["remote", "set-head", remote, "--delete"]);
        self.assert_no_remote_head(remote);
    }
    fn assert_no_remote_head(&self, remote: &str) {
        let out = self
            .command(
                "git",
                &self.main,
                &[
                    "symbolic-ref",
                    "--quiet",
                    &format!("refs/remotes/{remote}/HEAD"),
                ],
            )
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{out:?}");
    }
    fn rabi(&self, path: &Path, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_rabi"), path, args)
            .output()
            .unwrap()
    }
    fn sync(&self, path: &Path, args: &[&str], code: i32) -> Value {
        let mut all = vec!["sync", "--json"];
        all.extend_from_slice(args);
        let out = self.rabi(path, &all);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn create(&self, name: &str) -> PathBuf {
        let out = self.rabi(&self.main, &["create", name, "--json"]);
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            doc["data"]["workspace"]["base"],
            doc["data"]["source"]["base"]
        );
        PathBuf::from(doc["data"]["workspace"]["path"].as_str().unwrap())
    }
    fn write_commit(&self, path: &Path, file: &str, text: &str) {
        fs::write(path.join(file), text).unwrap();
        self.git(path, &["add", file]);
        self.git(path, &["commit", "-m", file]);
    }
    fn install_fsmonitor(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let marker = self.root.join("fsmonitor-ran");
        let hook = self.root.join("fsmonitor-hook");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nprintf '/\\000'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        self.git(
            &self.main,
            &["config", "core.fsmonitor", hook.to_str().unwrap()],
        );
        self.git(&self.main, &["status", "--porcelain"]);
        assert!(marker.exists());
        fs::remove_file(&marker).unwrap();
        marker
    }
    fn advance(&self) {
        self.write_commit(&self.upstream, "remote-file", "remote\n");
        self.git(&self.upstream, &["push", "origin", "main"]);
    }
    fn snapshot(&self, path: &Path) -> Value {
        json!({"head": self.git(path, &["rev-parse", "HEAD"]), "status": self.git(path, &["status", "--porcelain=v2"]), "index": self.git(path, &["write-tree"]), "files": file_contents(path)})
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn check_primary_sync_selectors(default_branch: bool, with_main: bool) {
    // Design §4: primary selectors use --main semantics; non-default primary skips.
    for strategy in [None, Some("--rebase"), Some("--merge")] {
        for named in [false, true] {
            let r = Repo::new();
            if !default_branch {
                r.git(&r.main, &["checkout", "-b", "primary-topic"]);
                r.write_commit(&r.main, "local-file", "local\n");
            }
            let shown = r.rabi(&r.main, &["show", "--json"]);
            let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
            let name = shown["data"]["workspace"]["name"].as_str().unwrap();
            let before = r.snapshot(&r.main);
            let default_before = r.git(&r.main, &["rev-parse", "main"]);
            r.advance();
            let mut args = vec![if named { name } else { "." }];
            if with_main {
                args.push("--main");
            }
            if let Some(strategy) = strategy {
                args.push(strategy);
            }
            let doc = r.sync(&r.main, &args, 0);
            assert_eq!(doc["schema"], 2);
            assert_eq!(doc["data"]["items"], json!([]));
            assert_eq!(doc["data"]["main"]["strategy"], "ff-only");
            assert_eq!(
                doc["data"]["main"]["status"],
                if default_branch {
                    "updated"
                } else {
                    "not-default-branch"
                }
            );
            if default_branch {
                assert_eq!(
                    r.git(&r.main, &["rev-parse", "HEAD"]),
                    r.git(&r.upstream, &["rev-parse", "HEAD"])
                );
            } else {
                assert_eq!(r.snapshot(&r.main), before);
                assert_eq!(r.git(&r.main, &["rev-parse", "main"]), default_before);
            }
        }
    }
}

#[test]
fn primary_nondefault_sync_selectors_skip_integration() {
    check_primary_sync_selectors(false, false);
}

#[test]
fn primary_nondefault_sync_selectors_skip_integration_with_main() {
    check_primary_sync_selectors(false, true);
}

#[test]
fn primary_default_sync_selectors_only_fast_forward() {
    check_primary_sync_selectors(true, false);
}

#[test]
fn primary_default_sync_selectors_with_main_only_fast_forward() {
    check_primary_sync_selectors(true, true);
}

#[test]
fn primary_default_sync_selectors_never_integrate_divergence() {
    for no_default in [false, true] {
        for selector in [".", "main"] {
            for strategy in ["--rebase", "--merge"] {
                let r = Repo::new();
                r.write_commit(&r.main, "local-file", "local\n");
                let before = r.snapshot(&r.main);
                r.advance();
                let mut args = vec![selector, strategy];
                if no_default {
                    args.push("--main");
                }
                let doc = r.sync(&r.main, &args, 1);
                assert_eq!(r.snapshot(&r.main), before, "{args:?}");
                assert_eq!(doc["data"]["items"], json!([]), "{doc}");
                // Design §4: explicit primary selectors always use ff-only.
                {
                    assert_eq!(doc["data"]["main"]["strategy"], "ff-only");
                    assert_eq!(doc["data"]["main"]["status"], "diverged");
                }
            }
        }
    }
}

#[test]
fn default_upstream_precedes_other_remote_heads_without_origin_head() {
    for existing_trunk in [false, true] {
        let r = Repo::new();
        r.git(&r.main, &["remote", "set-head", "origin", "--delete"]);
        r.git(
            &r.main,
            &["branch", "--set-upstream-to=origin/main", "main"],
        );
        let second = r.root.join("second.git");
        r.git(
            &r.main,
            &[
                "clone",
                "--bare",
                r.root.join("remote.git").to_str().unwrap(),
                second.to_str().unwrap(),
            ],
        );
        r.git(&second, &["branch", "trunk", "main"]);
        r.git(&second, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        r.git(
            &r.main,
            &["remote", "add", "upstream", second.to_str().unwrap()],
        );
        r.git(&r.main, &["fetch", "upstream"]);
        r.git(&r.main, &["remote", "set-head", "upstream", "trunk"]);
        let second_before = r.git(&r.main, &["for-each-ref", "refs/remotes/upstream"]);
        if existing_trunk {
            r.git(&r.main, &["branch", "trunk", "main"]);
        }
        let trunk_before = r.git(&r.main, &["for-each-ref", "refs/heads/trunk"]);
        // Make the second remote stale too: sync must fetch only origin.
        r.write_commit(&r.upstream, "second-only", "second remote\n");
        r.git(
            &r.upstream,
            &["push", second.to_str().unwrap(), "main:trunk"],
        );
        r.git(&r.upstream, &["reset", "--hard", "origin/main"]);
        r.advance();
        let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
        let doc = r.sync(&r.main, &[], 0);
        assert_eq!(doc["data"]["main"]["status"], "updated", "{doc}");
        assert_eq!(doc["data"]["main"]["branch"], "refs/heads/main");
        assert_eq!(doc["data"]["main"]["base"], "refs/remotes/origin/main");
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), expected);
        assert_eq!(r.git(&r.main, &["rev-parse", "origin/main"]), expected);
        assert_eq!(
            r.git(&r.main, &["for-each-ref", "refs/heads/trunk"]),
            trunk_before
        );
        assert_eq!(
            r.git(&r.main, &["for-each-ref", "refs/remotes/upstream"]),
            second_before
        );
        assert_eq!(r.git(&r.main, &["rev-parse", "main@{upstream}"]), expected);
        assert_eq!(
            r.sync(&r.main, &[], 0)["data"]["main"]["status"],
            "up-to-date"
        );
    }
}

#[test]
fn custom_namespace_origin_head_precedes_other_remote_heads() {
    for (refspec, destination) in [
        (
            "+refs/heads/*:refs/remotes/cache/*",
            "refs/remotes/cache/main",
        ),
        (
            "+refs/heads/*:refs/remotes/custom/*",
            "refs/remotes/custom/main",
        ),
    ] {
        let r = Repo::new();
        r.git(&r.main, &["config", "remote.origin.fetch", refspec]);
        r.git(&r.main, &["fetch", "origin"]);
        r.git(
            &r.main,
            &["symbolic-ref", "refs/remotes/origin/HEAD", destination],
        );
        r.git(
            &r.main,
            &[
                "remote",
                "add",
                "fork",
                r.root.join("remote.git").to_str().unwrap(),
            ],
        );
        r.git(&r.main, &["fetch", "fork"]);
        // Explicitly create the HEAD on Git < 2.48 too.
        r.git(&r.main, &["remote", "set-head", "fork", "main"]);
        assert_eq!(r.git(&r.main, &["remote"]), "fork\norigin");
        let fork_before = r.git(&r.main, &["for-each-ref", "refs/remotes/fork"]);
        r.advance();
        let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);

        let doc = r.sync(&r.main, &[], 0);
        assert_eq!(doc["data"]["main"]["base"], destination, "{doc}");
        assert_eq!(doc["data"]["main"]["status"], "updated");
        assert_eq!(doc["data"]["main"]["branch"], "refs/heads/main");
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), expected);
        assert_eq!(r.git(&r.main, &["rev-parse", destination]), expected);
        assert_eq!(
            r.git(&r.main, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
            destination
        );
        assert_eq!(
            r.git(&r.main, &["for-each-ref", "refs/remotes/fork"]),
            fork_before
        );
    }
}

#[test]
fn remote_head_fallback_preserves_remaining_remote_order() {
    for origin in ["absent", "missing-head", "dangling-head"] {
        let r = Repo::new();
        match origin {
            "absent" => {
                r.git(&r.main, &["remote", "remove", "origin"]);
            }
            "missing-head" => r.keep_remote_head_absent("origin"),
            "dangling-head" => {
                r.git(
                    &r.main,
                    &[
                        "symbolic-ref",
                        "refs/remotes/origin/HEAD",
                        "refs/remotes/origin/missing",
                    ],
                );
            }
            _ => unreachable!(),
        }
        for remote in ["fork", "healthy"] {
            r.git(
                &r.main,
                &[
                    "remote",
                    "add",
                    remote,
                    r.root.join("remote.git").to_str().unwrap(),
                ],
            );
            r.git(&r.main, &["fetch", remote]);
            r.git(&r.main, &["remote", "set-head", remote, "main"]);
        }
        let healthy_before = r.git(&r.main, &["for-each-ref", "refs/remotes/healthy"]);
        r.advance();
        let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
        let doc = r.sync(&r.main, &[], 0);
        assert_eq!(
            doc["data"]["main"]["base"], "refs/remotes/fork/main",
            "{origin}: {doc}"
        );
        assert_eq!(doc["data"]["main"]["status"], "updated");
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), expected);
        assert_eq!(
            r.git(&r.main, &["for-each-ref", "refs/remotes/healthy"]),
            healthy_before
        );
    }
}

#[test]
fn other_remote_head_remains_a_fallback_for_unresolved_default_upstream() {
    let r = Repo::new();
    let before = r.git(&r.main, &["rev-parse", "main"]);
    r.git(&r.main, &["remote", "set-head", "origin", "--delete"]);
    r.git(&r.main, &["config", "branch.main.remote", "origin"]);
    r.git(
        &r.main,
        &["config", "branch.main.merge", "refs/heads/missing"],
    );
    r.git(&r.upstream, &["push", "origin", "main:trunk"]);
    r.git(
        &r.main,
        &[
            "remote",
            "add",
            "upstream",
            r.root.join("remote.git").to_str().unwrap(),
        ],
    );
    r.git(&r.main, &["fetch", "upstream"]);
    r.git(&r.main, &["remote", "set-head", "upstream", "trunk"]);
    r.advance();
    r.git(&r.upstream, &["push", "origin", "main:trunk"]);
    let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
    let doc = r.sync(&r.main, &[], 0);
    assert_eq!(doc["data"]["main"]["status"], "not-default-branch", "{doc}");
    assert_eq!(doc["data"]["main"]["base"], "refs/remotes/upstream/trunk");
    assert_eq!(r.git(&r.main, &["branch", "--list", "trunk"]), "");
    let _ = expected;
    assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
    assert_eq!(
        r.git(&r.main, &["config", "branch.main.merge"]),
        "refs/heads/missing"
    );
}

#[test]
fn default_upstream_ignores_child_branch_without_remote_head() {
    for has_upstream in [false, true] {
        let r = Repo::new();
        r.keep_remote_head_absent("origin");
        let before = r.git(&r.main, &["rev-parse", "main"]);
        r.git(&r.upstream, &["push", "origin", "main:trunk"]);
        r.advance();
        r.git(&r.upstream, &["push", "origin", "main:child"]);
        r.git(&r.main, &["fetch", "origin"]);
        let child_tip = r.git(&r.main, &["rev-parse", "origin/main"]);
        r.git(
            &r.main,
            &["update-ref", "refs/remotes/origin/child", &child_tip],
        );
        r.git(
            &r.main,
            &["update-ref", "refs/remotes/origin/main", &before],
        );
        // A packed-ref fixture represents the prefix collision that Git's normal
        // branch creation rejects. Both refs remain readable by ref enumeration.
        r.git(&r.main, &["pack-refs", "--all"]);
        let packed = r.main.join(".git/packed-refs");
        let mut refs = fs::read_to_string(&packed).unwrap();
        refs.push_str(&format!("{child_tip} refs/heads/main/foo\n"));
        let mut lines: Vec<_> = refs.lines().collect();
        lines.sort_by_key(|line| line.split_once(' ').map(|(_, name)| name).unwrap_or(line));
        fs::write(packed, lines.join("\n") + "\n").unwrap();
        r.git(&r.main, &["config", "branch.main/foo.remote", "origin"]);
        r.git(
            &r.main,
            &["config", "branch.main/foo.merge", "refs/heads/child"],
        );
        if has_upstream {
            r.git(
                &r.main,
                &["update-ref", "refs/remotes/origin/trunk", &before],
            );
            r.git(&r.main, &["config", "branch.main.remote", "origin"]);
            r.git(
                &r.main,
                &["config", "branch.main.merge", "refs/heads/trunk"],
            );
        }
        // Keep the deliberately distinct tracking tips unchanged during sync.
        r.git(
            &r.main,
            &["config", "--add", "remote.origin.fetch", "^refs/heads/main"],
        );
        assert_eq!(
            r.git(&r.main, &["rev-parse", "refs/heads/main/foo"]),
            child_tip
        );
        r.assert_no_remote_head("origin");
        let doc = r.sync(&r.main, &[], 0);
        r.assert_no_remote_head("origin");
        assert_eq!(doc["data"]["main"]["status"], "up-to-date");
        assert_eq!(
            doc["data"]["main"]["base"],
            if has_upstream {
                "refs/remotes/origin/trunk"
            } else {
                "refs/remotes/origin/main"
            }
        );
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
    }
}

#[test]
fn default_without_remote_head_uses_differently_named_upstream() {
    for tracking in [
        "refs/remotes/origin/trunk",
        "refs/remotes/origin/mapped/trunk",
    ] {
        for checked_out in [true, false] {
            let r = Repo::new();
            r.keep_remote_head_absent("origin");
            r.git(&r.upstream, &["push", "origin", "main:trunk"]);
            if tracking.contains("/mapped/") {
                r.git(
                    &r.main,
                    &[
                        "config",
                        "remote.origin.fetch",
                        "+refs/heads/*:refs/remotes/origin/mapped/*",
                    ],
                );
            }
            r.git(&r.main, &["fetch", "origin"]);
            r.git(&r.main, &["config", "branch.main.remote", "origin"]);
            r.git(
                &r.main,
                &["config", "branch.main.merge", "refs/heads/trunk"],
            );
            if !checked_out {
                r.git(&r.main, &["update-ref", "-d", "refs/remotes/origin/main"]);
            }
            // Neither a missing nor a stale origin/main may override the upstream.
            r.assert_no_remote_head("origin");
            let tree = r.create("topic");
            let doc = r.sync(&tree, &[], 0);
            assert_eq!(doc["data"]["items"][0]["base"], tracking);
            if !checked_out {
                r.git(&r.main, &["checkout", "-b", "other"]);
            }
            r.write_commit(&r.upstream, "remote-trunk", "advanced\n");
            r.git(&r.upstream, &["push", "origin", "main:trunk"]);
            let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
            assert_ne!(r.git(&r.main, &["rev-parse", tracking]), expected);
            let doc = r.sync(&r.main, &[], 0);
            if !checked_out {
                assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
                // Design §3.4: only primary on default can be advanced by --main.
                r.git(&r.main, &["checkout", "main"]);
                r.sync(&r.main, &[], 0);
            } else {
                assert_eq!(doc["data"]["main"]["status"], "updated");
            }
            if checked_out {
                assert_eq!(doc["data"]["main"]["branch"], "refs/heads/main");
            }
            assert_eq!(doc["data"]["main"]["base"], tracking);
            assert_eq!(r.git(&r.main, &["rev-parse", "main"]), expected);
            assert_eq!(r.git(&r.main, &["rev-parse", tracking]), expected);
            r.assert_no_remote_head("origin");
            assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads/trunk"]), "");
            assert_eq!(
                r.sync(&r.main, &[], 0)["data"]["main"]["status"],
                "up-to-date"
            );
        }
    }
}

#[test]
fn default_branch_identity_survives_remote_head_creation_during_fetch() {
    use std::os::unix::fs::PermissionsExt;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("git"))
        .find(|path| path.is_file())
        .unwrap();
    for checkout in ["primary", "unchecked", "linked"] {
        for existing_trunk in [false, true] {
            let r = Repo::new();
            r.git(&r.upstream, &["push", "origin", "main:trunk"]);
            r.git(&r.main, &["fetch", "origin"]);
            r.git(
                &r.main,
                &["branch", "--set-upstream-to=origin/trunk", "main"],
            );
            if existing_trunk {
                r.git(&r.main, &["branch", "trunk", "main"]);
            }
            let trunk_before = r.git(&r.main, &["for-each-ref", "refs/heads/trunk"]);
            if checkout != "primary" {
                r.git(&r.main, &["checkout", "-b", "other"]);
            }
            let linked = r.root.join("linked-default");
            if checkout == "linked" {
                r.git(
                    &r.main,
                    &["worktree", "add", linked.to_str().unwrap(), "main"],
                );
            }
            r.advance();
            r.git(&r.upstream, &["push", "origin", "main:trunk"]);
            let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
            r.git(&r.main, &["remote", "set-head", "origin", "--delete"]);
            r.assert_no_remote_head("origin");

            // Emulate Git >= 2.48 creating origin/HEAD during rabi's fetch,
            // so older Git also exercises the change between planning and use.
            let bin = r.root.join("bin");
            fs::create_dir(&bin).unwrap();
            let wrapper = bin.join("git");
            fs::write(
                &wrapper,
                "#!/bin/sh\n\"$REAL_GIT\" \"$@\" || exit $?\ncase \" $* \" in\n  *' fetch '*) \"$REAL_GIT\" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main ;;\nesac\n",
            )
            .unwrap();
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
            let mut paths = vec![bin];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
            let cwd = if checkout == "linked" {
                &linked
            } else {
                &r.main
            };
            let args = if checkout == "linked" {
                vec!["sync", ".", "--json"]
            } else {
                vec!["sync", "--json"]
            };
            let out = r
                .command(env!("CARGO_BIN_EXE_rabi"), cwd, &args)
                .env("PATH", std::env::join_paths(paths).unwrap())
                .env("REAL_GIT", &real_git)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
            if checkout == "unchecked" {
                // Design §3.4: primary is on other; no unselected ref is advanced.
                assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
                assert_ne!(r.git(&r.main, &["rev-parse", "main"]), expected);
                continue;
            }
            let item = if checkout == "linked" {
                assert!(doc["data"]["main"].is_null());
                &doc["data"]["items"][0]
            } else {
                &doc["data"]["main"]
            };
            assert_eq!(item["status"], "updated", "{doc}");
            assert_eq!(item["branch"], "refs/heads/main", "{doc}");
            assert_eq!(item["base"], "refs/remotes/origin/trunk");
            assert_eq!(r.git(&r.main, &["rev-parse", "main"]), expected);
            assert_eq!(
                r.git(&r.main, &["for-each-ref", "refs/heads/trunk"]),
                trunk_before
            );
            assert_eq!(
                r.git(&r.main, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
                "refs/remotes/origin/main"
            );
            assert_eq!(
                r.git(&r.main, &["config", "branch.main.merge"]),
                "refs/heads/trunk"
            );
        }
    }
}

#[test]
fn implicit_sync_does_not_revalidate_the_resolved_branch_name() {
    for subdirectory in [false, true] {
        let r = Repo::new();
        let tree = r.root.join("plain-checkout");
        r.git(
            &r.main,
            &[
                "worktree",
                "add",
                "-b",
                "topic+fix",
                tree.to_str().unwrap(),
                "main",
            ],
        );
        let other = r.create("other");
        let other_before = r.snapshot(&other);
        let store = r.main.join(".git/rabi/workspaces");
        let records_before = records::read(&store).unwrap();
        r.advance();
        let cwd = if subdirectory {
            let sub = tree.join("sub");
            fs::create_dir(&sub).unwrap();
            sub
        } else {
            tree.clone()
        };
        let doc = r.sync(&cwd, &[], 0);
        assert_eq!(doc["schema"], 2);
        // Design §4: implicit workspace sync leaves primary alone.
        assert!(doc["data"]["main"].is_null());
        assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 1);
        assert_eq!(doc["data"]["items"][0]["name"], "topic+fix");
        assert_eq!(doc["data"]["items"][0]["status"], "updated");
        assert_eq!(
            r.git(&tree, &["rev-parse", "HEAD"]),
            r.git(&r.upstream, &["rev-parse", "HEAD"])
        );
        assert_eq!(r.snapshot(&other), other_before);
        assert_eq!(records::read(&store).unwrap(), records_before);
    }
}

#[test]
fn default_only_and_linked_implicit_including_subdirectory_and_detached() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "local-file", "local\n");
    let before = r.git(&tree, &["rev-parse", "HEAD"]);
    r.advance();
    let doc = r.sync(&r.main, &[], 0);
    assert_eq!(doc["data"]["main"]["status"], "updated");
    assert_eq!(doc["data"]["items"], json!([]));
    assert_eq!(r.git(&tree, &["rev-parse", "HEAD"]), before);
    fs::create_dir(tree.join("sub")).unwrap();
    let doc = r.sync(&tree.join("sub"), &[], 0);
    assert_eq!(doc["data"]["items"][0]["status"], "updated");
    assert_eq!(doc["data"]["items"][0]["strategy"], "rebase");
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "up-to-date"
    );
    r.git(&tree, &["checkout", "--detach"]);
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "detached"
    );
}

#[test]
fn linked_default_is_only_integrated_fast_forward_once() {
    // Design §3.4: primary skips when on another branch; selected linked default
    // still uses the existing ff-only guard regardless of strategy/permission.
    for diverged in [false, true] {
        let r = Repo::new();
        r.git(&r.main, &["checkout", "-b", "other"]);
        let tree = r.root.join("default-tree");
        r.git(
            &r.main,
            &["worktree", "add", tree.to_str().unwrap(), "main"],
        );
        assert!(r.rabi(&tree, &["set", "--base", "other"]).status.success());
        if diverged {
            r.write_commit(&tree, "own-main", "local\n");
        }
        r.advance();
        let primary = r.snapshot(&r.main);
        let before = r.snapshot(&tree);
        let doc = r.sync(&tree, &["--main"], 0);
        assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
        assert_eq!(r.snapshot(&tree), before);
        for (i, args) in [
            vec![],
            vec!["--all"],
            vec!["main", "--rebase", "--rewrite-published"],
            vec!["main", "--merge", "--main"],
        ]
        .iter()
        .enumerate()
        {
            let doc = r.sync(&tree, args, 0);
            let items = doc["data"]["items"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["strategy"], "ff-only");
            assert_eq!(
                items[0]["status"],
                if diverged {
                    "diverged"
                } else if i == 0 {
                    "updated"
                } else {
                    "up-to-date"
                }
            );
            if i == 3 {
                assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
            } else {
                assert!(doc["data"]["main"].is_null());
            }
            assert_eq!(r.snapshot(&r.main), primary);
            if diverged {
                assert_eq!(r.snapshot(&tree), before);
            }
        }
    }
}

#[test]
fn default_ref_update_linked_checkout_diverged_and_dirty() {
    // Design §3.4: --main cannot update an unselected local/default checkout.
    let r = Repo::new();
    r.git(&r.main, &["checkout", "-b", "other"]);
    let default_before = r.git(&r.main, &["rev-parse", "main"]);
    r.advance();
    assert_eq!(
        r.sync(&r.main, &[], 0)["data"]["main"]["status"],
        "not-default-branch"
    );
    assert_eq!(r.git(&r.main, &["rev-parse", "main"]), default_before);
    let linked = r.root.join("default-tree");
    r.git(
        &r.main,
        &["worktree", "add", linked.to_str().unwrap(), "main"],
    );
    assert_eq!(
        r.sync(&linked, &[], 0)["data"]["items"][0]["status"],
        "updated"
    );
    r.write_commit(&linked, "own-main", "local\n");
    r.write_commit(&r.upstream, "remote-two", "two\n");
    r.git(&r.upstream, &["push", "origin", "main"]);
    let before = r.snapshot(&linked);
    assert_eq!(
        r.sync(&linked, &[], 0)["data"]["items"][0]["status"],
        "diverged"
    );
    assert_eq!(r.snapshot(&linked), before);
    let dirty = Repo::new();
    dirty.write_commit(&dirty.upstream, "file", "remote edit\n");
    dirty.git(&dirty.upstream, &["push", "origin", "main"]);
    fs::write(dirty.main.join("file"), "dirty edit\n").unwrap();
    let before = dirty.snapshot(&dirty.main);
    assert_eq!(
        dirty.sync(&dirty.main, &[], 1)["data"]["main"]["status"],
        "dirty"
    );
    assert_eq!(dirty.snapshot(&dirty.main), before);
}

#[test]
fn pushed_branch_containing_base_is_up_to_date_without_force() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.git(&tree, &["push", "origin", "topic"]);
    let before = r.snapshot(&tree);
    let head = r.git(&tree, &["rev-parse", "HEAD"]);
    for (args, strategy) in [
        (vec!["topic", "--rebase"], "rebase"),
        (vec!["topic"], "merge"),
        (vec!["topic", "--merge"], "merge"),
    ] {
        let doc = r.sync(&r.main, &args, 0);
        let item = &doc["data"]["items"][0];
        assert_eq!(item["status"], "up-to-date");
        assert_eq!(item["strategy"], strategy);
        assert_eq!(item["before"], head);
        assert_eq!(item["after"], head);
        assert_eq!(item["code"], Value::Null);
        assert_eq!(r.snapshot(&tree), before);
    }
}

#[test]
fn corrupt_custom_base_metadata_refuses_sync_before_fetch_or_updates() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.create("healthy");
    r.git(&r.main, &["branch", "stack"]);
    r.write_commit(&tree, "own", "own commit\n");
    assert!(r
        .rabi(&r.main, &["set", "topic", "--base", "stack"])
        .status
        .success());
    let dir = r.main.join(".git/rabi/workspaces");
    let record = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|file| {
            let record: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
            record["path"] == tree.to_str().unwrap()
        })
        .unwrap();
    let mut body: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(body["base"], "refs/heads/stack");
    body["purpose"] = json!({"invalid": "shape"});
    let corrupt = serde_json::to_vec(&body).unwrap();
    fs::write(&record, &corrupt).unwrap();
    r.advance();
    let refs = r.git(&r.main, &["for-each-ref"]);
    let before = r.snapshot(&tree);
    let main_before = r.snapshot(&r.main);
    let doc = r.sync(&r.main, &["topic", "healthy"], 1);
    assert_eq!(doc["error"]["code"], "failed");
    assert!(doc["error"]["message"]
        .as_str()
        .unwrap()
        .contains("invalid JSON or shape"));
    assert!(doc["data"].is_null());
    assert_eq!(r.git(&r.main, &["for-each-ref"]), refs);
    assert_eq!(r.snapshot(&tree), before);
    assert_eq!(r.snapshot(&r.main), main_before);
    assert_eq!(fs::read(&record).unwrap(), corrupt);
    let listed = r.rabi(&r.main, &["list", "--json"]);
    assert!(listed.status.success());
    let doc: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(!doc["warnings"].as_array().unwrap().is_empty());
    assert!(doc["data"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["name"] == "healthy" && item["id"].is_string()));
}

#[test]
fn updates_with_failures_conflicts_or_published_refusals_exit_partial() {
    for update_default in [true, false] {
        for outcome in ["conflict", "kept-conflict", "published", "failure"] {
            let r = Repo::new();
            let tree = r.create("topic");
            r.write_commit(&tree, "file", "topic edit\n");
            if outcome == "published" {
                r.git(&tree, &["push", "origin", "topic"]);
            }
            if outcome == "failure" {
                r.git(
                    &r.main,
                    &[
                        "remote",
                        "add",
                        "broken",
                        r.root.join("remote.git").to_str().unwrap(),
                    ],
                );
                r.git(&r.main, &["fetch", "broken"]);
                assert!(r
                    .rabi(&r.main, &["set", "topic", "--base", "broken/main"])
                    .status
                    .success());
                r.git(
                    &r.main,
                    &[
                        "remote",
                        "set-url",
                        "broken",
                        r.root.join("absent.git").to_str().unwrap(),
                    ],
                );
            }
            let good = r.create("good");
            r.write_commit(&good, "own", "independent edit\n");
            r.write_commit(&r.upstream, "file", "remote edit\n");
            r.git(&r.upstream, &["push", "origin", "main"]);
            let before = r.snapshot(&tree);
            let mut args = vec!["topic", "--rebase", "--main"];
            if !update_default {
                // Deliberately update the other item before visiting topic.
                args = vec!["good", "topic", "--rebase"];
            }
            if outcome == "kept-conflict" {
                // Existing single-workspace rule: --main is ff-only (§3.4).
                if !update_default {
                    continue;
                }
                args.push("--keep-conflicts");
            }
            let doc = r.sync(&r.main, &args, 6);
            assert_eq!(doc["error"]["code"], "partial");
            let topic = doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["name"] == "topic")
                .unwrap();
            if outcome == "published" {
                assert_eq!(topic["code"], "published");
            } else if outcome == "failure" {
                assert_eq!(topic["status"], "failed");
            } else {
                assert_eq!(topic["status"], "conflict");
            }
            if outcome != "kept-conflict" {
                assert_eq!(r.snapshot(&tree), before);
            } else {
                assert!(r.git(&tree, &["status", "--porcelain=v2"]).contains("u "));
            }
            let updated = if update_default {
                &doc["data"]["main"]
            } else {
                &doc["data"]["items"][0]
            };
            assert_eq!(updated["status"], "updated");
            assert_ne!(updated["before"], updated["after"]);
        }
    }
}

#[test]
fn pushed_auto_merge_and_rebase_refusal_force_preserve_upstream() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    let before = r.git(&tree, &["rev-parse", "HEAD"]);
    r.git(&tree, &["push", "-u", "origin", "topic"]);
    r.advance();
    let refused = r.sync(&r.main, &["topic", "--rebase"], 4);
    assert_eq!(refused["error"]["code"], "published");
    assert_eq!(refused["data"]["items"][0]["code"], "published");
    assert_eq!(r.git(&tree, &["rev-parse", "HEAD"]), before);
    let forced = r.sync(&r.main, &["topic", "--rebase", "--rewrite-published"], 0);
    assert_eq!(forced["data"]["items"][0]["strategy"], "rebase");
    assert_ne!(r.git(&tree, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        r.git(&tree, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/topic"
    );
    let s = Repo::new();
    let tree = s.create("pushed");
    s.write_commit(&tree, "own", "own\n");
    s.git(&tree, &["push", "origin", "pushed"]);
    s.advance();
    let doc = s.sync(&s.main, &["pushed"], 0);
    assert_eq!(doc["data"]["items"][0]["strategy"], "merge");
    assert_eq!(
        s.git(&tree, &["log", "-1", "--format=%s"]),
        "Merge origin/main into pushed"
    );
    assert_eq!(
        s.git(&tree, &["rev-list", "--parents", "-n", "1", "HEAD"])
            .split_whitespace()
            .count(),
        3
    );
}

#[test]
fn conflicts_restore_everything_and_keep_conflicts_is_single_target_only() {
    for strategy in ["--rebase", "--merge"] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "file", "local edit\n");
        r.write_commit(&r.upstream, "file", "remote edit\n");
        r.git(&r.upstream, &["push", "origin", "main"]);
        fs::write(tree.join("ignored"), "ignored contents\n").unwrap();
        r.git(
            &tree,
            &[
                "config",
                "core.excludesFile",
                r.root.join("ignore").to_str().unwrap(),
            ],
        );
        fs::write(r.root.join("ignore"), "ignored\n").unwrap();
        let before = r.snapshot(&tree);
        let doc = r.sync(&tree, &[strategy], 5);
        assert_eq!(doc["data"]["items"][0]["status"], "conflict");
        assert_eq!(doc["data"]["items"][0]["conflicts"], json!(["file"]));
        assert_eq!(r.snapshot(&tree), before);
        assert_eq!(
            fs::read(tree.join("ignored")).unwrap(),
            b"ignored contents\n"
        );
        let doc = r.sync(&tree, &[strategy, "--keep-conflicts"], 5);
        assert_eq!(doc["data"]["items"][0]["status"], "conflict");
        assert!(r.git(&tree, &["status", "--porcelain=v2"]).contains("u "));
        r.git(
            &tree,
            &[
                if strategy == "--merge" {
                    "merge"
                } else {
                    "rebase"
                },
                "--abort",
            ],
        );
        r.create("second");
        r.sync(&r.main, &["topic", "second", "--keep-conflicts"], 2);
    }
}

#[test]
fn dirty_merged_squash_base_gone_and_metadata_base() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "topic-file", "topic\n");
    fs::write(tree.join("untracked"), "untracked\n").unwrap();
    assert_eq!(r.sync(&tree, &[], 0)["data"]["items"][0]["status"], "dirty");
    fs::remove_file(tree.join("untracked")).unwrap();
    r.git(&r.main, &["merge", "--squash", "topic"]);
    r.git(&r.main, &["commit", "-m", "squash"]);
    r.git(&r.main, &["push", "origin", "main"]);
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "integrated"
    );
    r.git(&r.main, &["branch", "stack"]);
    let out = r.rabi(&r.main, &["set", "topic", "--base", "stack", "--json"]);
    assert!(out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["workspace"]["base"], "refs/heads/stack");
    let invalid = r.rabi(&r.main, &["set", "topic", "--base", "nonexistent"]);
    assert!(!invalid.status.success());
    r.git(&r.main, &["branch", "-D", "stack"]);
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "base-gone"
    );
    let file = r.main.join(".git/rabi/workspaces");
    let mut stored: Value = serde_json::from_slice(&records::read(&file).unwrap()).unwrap();
    stored["worktrees"][0]
        .as_object_mut()
        .unwrap()
        .remove("base");
    records::write(&file, serde_json::to_vec(&stored).unwrap()).unwrap();
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["base"],
        "refs/remotes/origin/main"
    );
}

#[test]
fn all_syncs_recorded_and_unrecorded_workspaces() {
    let r = Repo::new();
    for name in ["first", "other"] {
        let tree = r.create(name);
        r.write_commit(&tree, name, "work\n");
    }
    let unrecorded = r.root.join("unrecorded");
    r.git(
        &r.main,
        &[
            "worktree",
            "add",
            "-b",
            "unrecorded",
            unrecorded.to_str().unwrap(),
        ],
    );
    r.write_commit(&unrecorded, "unrecorded", "work\n");
    r.advance();
    let out = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &r.main,
            &["sync", "--all", "--json"],
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(doc["data"]["main"].is_null());
    let trees = doc["data"]["items"].as_array().unwrap();
    assert_eq!(trees.len(), 3);
    for name in ["first", "other", "unrecorded"] {
        let tree = trees.iter().find(|tree| tree["name"] == name).unwrap();
        assert_eq!(tree["status"], "updated", "{tree}");
        assert!(tree.get("owner").is_none());
        assert!(tree.get("lastUsedAt").is_none());
    }
    r.write_commit(&r.upstream, "second-remote-file", "second remote change\n");
    r.git(&r.upstream, &["push", "origin", "main"]);
    let doc = r.sync(&r.main, &["other", "--merge"], 0);
    assert_eq!(doc["data"]["items"][0]["strategy"], "merge");
    assert_eq!(
        r.sync(&r.main, &["--all"], 0)["data"]["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn json_schema_snapshot_and_project_selection() {
    let r = Repo::new();
    let mut doc = r.sync(&r.main, &["--repo", r.main.to_str().unwrap()], 0);
    doc["data"]["main"]["path"] = json!("<path>");
    doc["data"]["main"]["before"] = json!("<sha>");
    doc["data"]["main"]["after"] = json!("<sha>");
    let workspace = doc["data"]["main"]
        .as_object_mut()
        .unwrap()
        .remove("workspace")
        .unwrap();
    assert_eq!(workspace["kind"], "primary");
    assert_eq!(workspace["name"], "main");
    assert_eq!(
        doc,
        json!({"schema":2,"command":"sync","ok":true,"warnings":[],"error":null,"data":{"plan":false,"main":{"name":"main","path":"<path>","branch":"refs/heads/main","base":"refs/remotes/origin/main","status":"up-to-date","strategy":"ff-only","strategySource":"default","rewriteAllowed":false,"rewriteSource":null,"rewrotePublished":false,"before":"<sha>","after":"<sha>","conflicts":[],"detail":null,"code":null},"items":[]}})
    );
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    assert_eq!(
        r.sync(&r.main, &["topic", "-R", r.main.to_str().unwrap()], 0)["data"]["items"][0]["name"],
        "topic"
    );
}

#[test]
fn fetch_only_involved_remotes_once_and_failed_fetch_cannot_update_stale_base() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.advance();
    r.git(&r.main, &["remote", "add", "unrelated", "/does/not/exist"]);
    let trace = r.root.join("trace");
    let out = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &r.main,
            &["sync", "topic", "--json"],
        )
        .env("GIT_TRACE", &trace)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let trace = fs::read_to_string(&trace).unwrap();
    assert_eq!(
        trace
            .matches("fetch --prune --no-auto-maintenance --no-recurse-submodules -- origin")
            .count(),
        1
    );
    assert!(
        !trace.contains("fetch --prune --no-auto-maintenance --no-recurse-submodules -- unrelated")
    );
    r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
    let before = r.snapshot(&tree);
    let doc = r.sync(&tree, &[], 1);
    assert_eq!(doc["data"]["items"][0]["status"], "failed");
    assert_eq!(r.snapshot(&tree), before);
}

#[test]
fn sync_runs_no_git_hooks_and_does_not_update_other_refs() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.git(&tree, &["branch", "other-ref"]);
    let old = r.git(&tree, &["rev-parse", "other-ref"]);
    let hooks = r.root.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let marker = r.root.join("hook-ran");
    for name in [
        "reference-transaction",
        "post-merge",
        "pre-rebase",
        "post-rewrite",
        "post-checkout",
    ] {
        let file = hooks.join(name);
        fs::write(&file, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    r.git(
        &r.main,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    r.git(&r.main, &["config", "rebase.updateRefs", "true"]);
    r.advance();
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "updated"
    );
    assert!(!marker.exists());
    assert_eq!(r.git(&tree, &["rev-parse", "other-ref"]), old);
    r.git(&r.main, &["checkout", "-b", "non-default"]);
    fs::remove_file(&marker).unwrap();
    r.write_commit(&r.upstream, "remote-more", "more\n");
    r.git(&r.upstream, &["push", "origin", "main"]);
    r.sync(&r.main, &[], 0);
    assert!(!marker.exists());
}

#[test]
fn ignored_collisions_follow_core_ignorecase() {
    for ignorecase in [true, false] {
        for strategy in ["--rebase", "--merge"] {
            for (ignored, incoming) in [
                ("Secret.txt", "secret.txt"),
                ("Build/x", "build"),
                ("Build", "build/x"),
                ("Build/x", "build/y"),
            ] {
                let r = Repo::new();
                let tree = r.create("topic");
                r.write_commit(&tree, "own", "own\n");
                r.git(
                    &tree,
                    &[
                        "config",
                        "core.ignorecase",
                        if ignorecase { "true" } else { "false" },
                    ],
                );
                let ignore = r.root.join("ignore");
                fs::write(&ignore, format!("{}\n", ignored.split('/').next().unwrap())).unwrap();
                r.git(
                    &tree,
                    &["config", "core.excludesFile", ignore.to_str().unwrap()],
                );
                let valuable = tree.join(ignored);
                fs::create_dir_all(valuable.parent().unwrap()).unwrap();
                fs::write(&valuable, "precious ignored data\n").unwrap();
                fs::create_dir_all(r.upstream.join(incoming).parent().unwrap()).unwrap();
                r.write_commit(&r.upstream, incoming, "remote tracked data\n");
                r.git(&r.upstream, &["push", "origin", "main"]);
                let before = r.snapshot(&tree);
                let doc = r.sync(&tree, &[strategy], 0);
                let collision = ignorecase;
                assert_eq!(
                    doc["data"]["items"][0]["status"],
                    if collision { "dirty" } else { "updated" },
                    "ignorecase={ignorecase}, {strategy}, {ignored}, {incoming}"
                );
                if collision {
                    assert_eq!(r.snapshot(&tree), before);
                }
                assert_eq!(fs::read(&valuable).unwrap(), b"precious ignored data\n");
            }
        }
    }
}

#[test]
fn ignored_files_are_preserved_when_base_would_overwrite_them() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    let ignore = r.root.join("ignore");
    fs::write(&ignore, "valuable\n").unwrap();
    r.git(
        &tree,
        &["config", "core.excludesFile", ignore.to_str().unwrap()],
    );
    fs::write(tree.join("valuable"), "precious ignored data\n").unwrap();
    r.write_commit(&r.upstream, "valuable", "remote tracked data\n");
    r.git(&r.upstream, &["push", "origin", "main"]);
    let before = r.snapshot(&tree);
    assert_eq!(r.sync(&tree, &[], 0)["data"]["items"][0]["status"], "dirty");
    assert_eq!(r.snapshot(&tree), before);
    assert_eq!(
        fs::read(tree.join("valuable")).unwrap(),
        b"precious ignored data\n"
    );
}

fn file_contents(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn collect(root: &Path, dir: &Path, files: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path == root.join(".git") {
                continue;
            }
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    collect(root, root, &mut files);
    files
}

#[test]
fn multiple_bases_fetch_each_involved_remote_and_respect_no_default() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    let remote = r.root.join("stack.git");
    r.git(
        &r.main,
        &[
            "clone",
            "--bare",
            r.main.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    r.git(
        &r.main,
        &["remote", "add", "stack", remote.to_str().unwrap()],
    );
    r.git(&r.main, &["fetch", "stack"]);
    assert!(r
        .rabi(&r.main, &["set", "topic", "--base", "stack/main"])
        .status
        .success());
    r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
    let doc = r.sync(&tree, &[], 0);
    assert_eq!(doc["data"]["items"][0]["base"], "refs/remotes/stack/main");
    r.git(
        &r.main,
        &[
            "remote",
            "set-url",
            "origin",
            r.root.join("remote.git").to_str().unwrap(),
        ],
    );
    let trace = r.root.join("trace");
    let out = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &r.main,
            &["sync", "topic", "--main", "--json"],
        )
        .env("GIT_TRACE", &trace)
        .output()
        .unwrap();
    assert!(out.status.success());
    let trace = fs::read_to_string(trace).unwrap();
    assert_eq!(
        trace
            .matches("fetch --prune --no-auto-maintenance --no-recurse-submodules -- origin")
            .count(),
        1
    );
    assert_eq!(
        trace
            .matches("fetch --prune --no-auto-maintenance --no-recurse-submodules -- stack")
            .count(),
        1
    );
}

#[test]
fn dirty_staged_and_unstaged_worktrees_are_untouched() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.advance();
    fs::write(tree.join("file"), "local edit\n").unwrap();
    for staged in [false, true] {
        if staged {
            r.git(&tree, &["add", "file"]);
        }
        let before = r.snapshot(&tree);
        assert_eq!(r.sync(&tree, &[], 0)["data"]["items"][0]["status"], "dirty");
        assert_eq!(r.snapshot(&tree), before);
    }
}

#[test]
fn separate_git_directory_default_checkout_and_published_squash_are_supported() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.git(&tree, &["push", "origin", "topic"]);
    r.git(&r.upstream, &["fetch", "origin"]);
    r.git(&r.upstream, &["merge", "--squash", "origin/topic"]);
    r.git(&r.upstream, &["commit", "-m", "squash"]);
    r.git(&r.upstream, &["push", "origin", "main"]);
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "integrated"
    );
    // Use a fresh repository: Git does not relocate linked-worktree gitfiles on re-init.
    let s = Repo::new();
    s.git(
        &s.main,
        &[
            "init",
            "--separate-git-dir",
            s.root.join("git-dir").to_str().unwrap(),
        ],
    );
    s.advance();
    assert_eq!(s.sync(&s.main, &[], 0)["data"]["main"]["status"], "updated");
}

#[test]
fn human_output_and_usage_errors() {
    let r = Repo::new();
    let out = r.rabi(&r.main, &["sync"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("main  up-to-date"));
    assert!(lines[1].starts_with("Sync complete:"));
    for args in [
        vec!["--all", "--mine"],
        vec!["--all", "topic"],
        vec!["--rebase", "--merge"],
        vec!["--keep-conflicts"],
        vec!["--bad-flag"],
    ] {
        r.sync(&r.main, &args, 2);
    }
}

#[test]
fn fresh_branches_update_but_fast_forward_landed_branches_are_merged() {
    let r = Repo::new();
    let tree = r.create("fresh");
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "up-to-date"
    );
    r.advance();
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "updated"
    );
    assert_eq!(
        r.git(&tree, &["rev-parse", "HEAD"]),
        r.git(&r.main, &["rev-parse", "origin/main"])
    );
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "up-to-date"
    );
    let gc = r.rabi(&r.main, &["gc", "--json"]);
    assert!(gc.status.success());
    let doc: Value = serde_json::from_slice(&gc.stdout).unwrap();
    assert!(!doc.to_string().contains("fresh"), "{doc}");
    let landed = r.create("landed");
    r.write_commit(&landed, "work", "work\n");
    r.git(&r.main, &["merge", "--ff-only", "landed"]);
    r.git(&r.main, &["push", "origin", "main"]);
    assert_eq!(
        r.sync(&landed, &[], 0)["data"]["items"][0]["status"],
        "integrated"
    );
    // Records without createdFrom use the very same reflog fallback as gc.
    let sparse = r.create("sparse");
    let file = r.main.join(".git/rabi/workspaces");
    let mut stored: Value = serde_json::from_slice(&records::read(&file).unwrap()).unwrap();
    for record in stored["worktrees"].as_array_mut().unwrap() {
        record.as_object_mut().unwrap().remove("createdFrom");
    }
    records::write(file, serde_json::to_vec(&stored).unwrap()).unwrap();
    r.write_commit(&r.upstream, "more", "more\n");
    r.git(&r.upstream, &["pull", "--no-rebase", "origin", "main"]);
    r.git(&r.upstream, &["push", "origin", "main"]);
    assert_eq!(
        r.sync(&sparse, &[], 0)["data"]["items"][0]["status"],
        "updated"
    );
}

#[test]
fn unrecorded_fresh_syncs_keep_null_metadata_and_gc_keeps_them() {
    for no_reflog in [false, true] {
        let r = Repo::new();
        // Use the managed directory so gc's directory boundary cannot hide a failure.
        let template = r.create("template");
        let tree = template.parent().unwrap().join("topic");
        if no_reflog {
            r.git(&r.main, &["config", "core.logAllRefUpdates", "false"]);
        }
        r.git(
            &r.main,
            &[
                "worktree",
                "add",
                "-b",
                "topic",
                tree.to_str().unwrap(),
                "origin/main",
            ],
        );
        let store = r.main.join(".git/rabi/workspaces");
        let before = records::read(&store).unwrap();
        r.advance();
        assert_eq!(
            r.sync(&r.main, &["topic"], 0)["data"]["items"][0]["status"],
            "updated"
        );
        r.write_commit(&r.upstream, "second", "second\n");
        r.git(&r.upstream, &["push", "origin", "main"]);
        assert_eq!(
            r.sync(&r.main, &["topic"], 0)["data"]["items"][0]["status"],
            "updated"
        );
        assert_eq!(
            r.sync(&r.main, &["topic"], 0)["data"]["items"][0]["status"],
            "up-to-date"
        );
        let gc = r.rabi(&r.main, &["gc", "--json"]);
        assert!(gc.status.success());
        let doc: Value = serde_json::from_slice(&gc.stdout).unwrap();
        assert!(!doc.to_string().contains("topic"), "{doc}");
        assert_eq!(records::read(store).unwrap(), before);
        if !no_reflog {
            r.write_commit(&r.upstream, "third", "third\n");
            r.git(&r.upstream, &["push", "origin", "main"]);
            r.git(&r.main, &["fetch", "origin"]);
            r.git(&tree, &["reset", "--hard", "origin/main"]);
            assert_eq!(
                r.sync(&r.main, &["topic"], 0)["data"]["items"][0]["status"],
                "integrated"
            );
            let gc = r.rabi(&r.main, &["gc", "--json"]);
            assert!(String::from_utf8_lossy(&gc.stdout).contains("topic"));
        }
    }
}

#[test]
fn ignored_directories_collapse_and_only_incoming_paths_block_sync() {
    for collision in [false, true] {
        let r = Repo::new();
        let tree = r.create("topic");
        let ignore = r.root.join("ignore");
        fs::write(&ignore, "cache/\n").unwrap();
        r.git(
            &tree,
            &["config", "core.excludesFile", ignore.to_str().unwrap()],
        );
        fs::create_dir(tree.join("cache")).unwrap();
        for i in 0..2000 {
            fs::write(tree.join(format!("cache/{i}")), "ignored data").unwrap();
        }
        if collision {
            // A tracked file replacing an ignored directory must be protected.
            r.write_commit(&r.upstream, "cache", "incoming file\n");
        } else {
            r.advance();
        }
        r.git(&r.upstream, &["push", "origin", "main"]);
        let doc = r.sync(&tree, &[], 0);
        assert_eq!(
            doc["data"]["items"][0]["status"],
            if collision { "dirty" } else { "updated" }
        );
        assert_eq!(
            fs::read_to_string(tree.join("cache/1999")).unwrap(),
            "ignored data"
        );
    }
}

#[test]
fn conflict_output_is_short_in_human_and_json_formats() {
    for strategy in ["rebase", "merge"] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "file", "local\n");
        r.write_commit(&r.upstream, "file", "remote\n");
        r.git(&r.upstream, &["push", "origin", "main"]);
        let flag = format!("--{strategy}");
        let doc = r.sync(&tree, &[&flag], 5);
        assert_eq!(
            doc["data"]["items"][0]["detail"],
            format!("{strategy} aborted; workspace unchanged")
        );
        let out = r.rabi(&tree, &["sync", &flag]);
        assert_eq!(out.status.code(), Some(5));
        assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("topic  conflict  file ({strategy} aborted; workspace unchanged)\nSync complete: 1 conflict\n"));
        let out = r.rabi(&tree, &["sync", &flag, "--keep-conflicts"]);
        assert_eq!(out.status.code(), Some(5));
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!(
                "topic  conflict  file ({strategy} left in progress)\nSync complete: 1 conflict\n"
            )
        );
    }
}

fn finish_child(mut child: std::process::Child) -> Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let out = child.wait_with_output().unwrap();
            panic!(
                "command did not finish: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn git_hooks_can_run_rabi() {
    use std::os::unix::fs::PermissionsExt;
    for hook_name in ["post-checkout", "reference-transaction"] {
        let r = Repo::new();
        r.git(&r.main, &["branch", "existing"]);
        let hook = r.main.join(".git/hooks").join(hook_name);
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        let status = r.root.join("nested-status");
        let output = r.root.join("nested-output");
        fs::write(
            &hook,
            format!(
                r#"#!/bin/sh
# Bound the nested command so a regression cannot leave orphaned child processes.
'{bin}' gc --json > '{output}' 2>&1 &
nested=$!
(sleep 3; kill "$nested" 2>/dev/null) >/dev/null 2>&1 &
watcher=$!
wait "$nested"
result=$?
kill "$watcher" 2>/dev/null
wait "$watcher" 2>/dev/null
echo "$result" >> '{status}'
exit "$result"
"#,
                bin = env!("CARGO_BIN_EXE_rabi"),
                output = output.display(),
                status = status.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        for args in [
            vec!["create", "topic", "--json"],
            vec!["create", "--branch", "existing", "--json"],
            vec![
                "remove",
                "topic",
                "--discard-changes",
                "--delete-unpreserved-branch",
                "--json",
            ],
        ] {
            let out = r.rabi(&r.main, &args);
            assert!(
                out.status.success(),
                "{args:?}: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let statuses = fs::read_to_string(&status).unwrap();
        assert!(!statuses.is_empty());
        assert!(
            statuses.lines().all(|line| line == "0"),
            "{hook_name}: {statuses}; {}",
            fs::read_to_string(output).unwrap()
        );
    }
}

#[test]
fn git_hook_can_run_create_with_project_hooks_and_standalone_exec() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    r.create("victim");
    let bin = env!("CARGO_BIN_EXE_rabi");
    let gate = r.root.join("hook-entered");
    let status = r.root.join("hook-status");
    let standalone = r.root.join("standalone-exec");
    let post_create = r.root.join("setup");
    let create_exec = r.root.join("create-exec");
    fs::write(
        r.main.join(".rabi.toml"),
        format!("[hooks]\nsetup = [\"touch '{}'\"]\n", post_create.display()),
    )
    .unwrap();
    let hook = r.main.join(".git/hooks/post-checkout");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        format!(
            r#"#!/bin/sh
[ -e '{gate}' ] && exit 0
touch '{gate}'
cd '{main}'
'{bin}' exec victim -- sh -c "touch '{standalone}'" &&
'{bin}' create nested &&
'{bin}' exec nested -- sh -c "touch '{create_exec}'"
echo $? > '{status}'
"#,
            gate = gate.display(),
            main = r.main.display(),
            standalone = standalone.display(),
            create_exec = create_exec.display(),
            status = status.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let child = r
        .command(bin, &r.main, &["create", "outer", "--no-setup"])
        .env("RABI_TRUST_ALL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish_child(child);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(status).unwrap().trim(),
        "0",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for marker in [standalone, post_create, create_exec] {
        assert!(marker.is_file(), "{}", marker.display());
    }
}

#[test]
fn post_create_hook_can_list_and_create_and_pre_delete_hook_can_create() {
    let r = Repo::new();
    let bin = env!("CARGO_BIN_EXE_rabi");
    fs::write(
        r.main.join(".rabi.toml"),
        format!(
            "[hooks]\nsetup = [\"'{bin}' list --json\", \"'{bin}' create nested --no-setup\"]\n"
        ),
    )
    .unwrap();
    let child = r
        .command(bin, &r.main, &["create", "outer"])
        .env("RABI_TRUST_ALL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish_child(child);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::write(
        r.main.join(".rabi.toml"),
        format!("[hooks]\npre-remove = [\"'{bin}' create from-delete --no-setup\"]\n"),
    )
    .unwrap();
    let child = r
        .command(
            bin,
            &r.main,
            &[
                "remove",
                "outer",
                "--discard-changes",
                "--delete-unpreserved-branch",
            ],
        )
        .env("RABI_TRUST_ALL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish_child(child);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(r.rabi(&r.main, &["path", "from-delete"]).status.success());
}

#[test]
fn other_commands_can_run_while_standalone_exec_is_running() {
    let r = Repo::new();
    let other = r.create("other");
    let gate = r.root.join("exec-gate");
    assert!(Command::new("mkfifo")
        .arg(&gate)
        .status()
        .unwrap()
        .success());
    let marker = r.root.join("exec-entered");
    let command = format!(
        "touch '{}'; read release < '{}'",
        marker.display(),
        gate.display()
    );
    r.create("agent");
    let mut create = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &r.main,
            &["exec", "agent", "--", "sh", "-c", &command],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !marker.exists() {
        assert!(
            create.try_wait().unwrap().is_none(),
            "create exited before exec"
        );
        assert!(std::time::Instant::now() < deadline, "exec did not start");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let delete = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &r.main,
            &[
                "remove",
                "other",
                "--discard-changes",
                "--delete-unpreserved-branch",
                "--no-hooks",
                "--json",
            ],
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deleted = finish_child(delete);
    assert!(
        deleted.status.success(),
        "{}",
        String::from_utf8_lossy(&deleted.stderr)
    );
    assert!(!other.exists());
    assert!(
        create.try_wait().unwrap().is_none(),
        "exec must still be waiting at gate"
    );
    fs::write(gate, "release\n").unwrap();
    assert!(finish_child(create).status.success());
}

#[test]
fn create_branch_and_gc_hooks_can_run_rabi_and_delete_rechecks_after_hooks() {
    let r = Repo::new();
    let bin = env!("CARGO_BIN_EXE_rabi");
    r.git(&r.main, &["branch", "existing"]);
    fs::write(
        r.main.join(".rabi.toml"),
        format!("[hooks]\nsetup = [\"'{bin}' create from-attach --no-setup\"]\n"),
    )
    .unwrap();
    let child = r
        .command(bin, &r.main, &["create", "--branch", "existing"])
        .env("RABI_TRUST_ALL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish_child(child);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::write(r.main.join(".rabi.toml"), "").unwrap();
    let tree = r.create("landed");
    r.write_commit(&tree, "work", "work\n");
    r.git(&r.main, &["merge", "--ff-only", "landed"]);
    r.git(&r.main, &["push", "origin", "main"]);
    fs::write(
        r.main.join(".rabi.toml"),
        format!("[hooks]\npre-remove = [\"'{bin}' create from-gc --no-setup\"]\n"),
    )
    .unwrap();
    let child = r
        .command(bin, &r.main, &["gc", "--merged", "--apply", "--json"])
        .env("RABI_TRUST_ALL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish_child(child);
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!tree.exists());
    assert!(r.rabi(&r.main, &["path", "from-gc"]).status.success());
    // A hook can change the target; the old preflight is stale.
    fs::write(
        r.main.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"git commit --allow-empty -m changed-by-hook\"]\n",
    )
    .unwrap();
    let child = r
        .command(
            bin,
            &r.main,
            &[
                "remove",
                "from-attach",
                "--discard-changes",
                "--delete-unpreserved-branch",
            ],
        )
        .env("RABI_TRUST_ALL", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = finish_child(child);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Workspace changed"));
    assert!(r.rabi(&r.main, &["path", "from-attach"]).status.success());
}

#[test]
fn rebase_checkout_preserves_ignored_paths_deleted_by_local_commits() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.git(&tree, &["rm", "file"]);
    r.git(&tree, &["commit", "-m", "delete file"]);
    let ignore = r.root.join("ignore");
    fs::write(&ignore, "file\n").unwrap();
    r.git(
        &tree,
        &["config", "core.excludesFile", ignore.to_str().unwrap()],
    );
    fs::write(tree.join("file"), "precious\n").unwrap();
    r.advance();
    assert_eq!(r.sync(&tree, &[], 0)["data"]["items"][0]["status"], "dirty");
    assert_eq!(fs::read_to_string(tree.join("file")).unwrap(), "precious\n");
}

#[test]
fn unrelated_default_history_fails_without_changing_the_checkout() {
    let r = Repo::new();
    r.git(&r.main, &["checkout", "--orphan", "replacement"]);
    r.git(&r.main, &["rm", "-rf", "."]);
    r.write_commit(&r.main, "unrelated", "local main\n");
    r.git(&r.main, &["branch", "-M", "main"]);
    let before = r.snapshot(&r.main);
    let refs = r.git(&r.main, &["show-ref"]);
    let doc = r.sync(&r.main, &[], 1);
    assert_eq!(doc["data"]["main"]["status"], "failed");
    assert!(doc["data"]["main"]["detail"]
        .as_str()
        .unwrap()
        .contains("no common history with refs/remotes/origin/main"));
    assert_eq!(r.snapshot(&r.main), before);
    assert_eq!(r.git(&r.main, &["show-ref"]), refs);
}

#[test]
fn silent_git_failures_are_checked_before_sync_mutations() {
    use std::os::unix::fs::PermissionsExt;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|file| file.is_file())
        .unwrap();
    for command in [
        "status",
        "ls-files",
        "log",
        "rev-list",
        "for-each-ref",
        "update-ref",
    ] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "local work\n");
        r.write_commit(&tree, ".gitignore", "valuable\n");
        fs::write(tree.join("valuable"), "precious\n").unwrap();
        let default_only = command == "update-ref";
        if default_only {
            r.git(&r.main, &["checkout", "-b", "other"]);
        }
        r.advance();
        // Keep the fetch outside the snapshot so only sync's integration is tested.
        r.git(&r.main, &["fetch", "origin"]);
        let before = r.snapshot(&tree);
        let main_before = r.snapshot(&r.main);
        let refs = r.git(&r.main, &["show-ref"]);
        let bin = r.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        fs::write(
            &wrapper,
            "#!/bin/sh\nfor arg do\n    if [ \"$arg\" = \"$FAIL_COMMAND\" ]; then exit 2; fi\ndone\nexec \"$REAL_GIT\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let args = vec!["sync", "--json"];
        let out = r
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                if default_only { &r.main } else { &tree },
                &args,
            )
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("REAL_GIT", &real_git)
            .env("FAIL_COMMAND", command)
            .output()
            .unwrap();
        // Design §3.4: --main cannot update a default ref outside primary.
        assert_eq!(
            out.status.code(),
            Some(if default_only { 0 } else { 1 }),
            "{command}: {out:?}"
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        let item = if default_only {
            &doc["data"]["main"]
        } else {
            &doc["data"]["items"][0]
        };
        if default_only {
            assert_eq!(item["status"], "not-default-branch");
        } else {
            assert_eq!(item["status"], "failed", "{command}: {doc}");
            assert!(item["detail"].as_str().unwrap().contains("exit 2"), "{doc}");
        }
        assert_eq!(r.snapshot(&tree), before, "{command}");
        assert_eq!(r.snapshot(&r.main), main_before, "{command}");
        assert_eq!(r.git(&r.main, &["show-ref"]), refs, "{command}");
    }
}

#[test]
fn unrelated_histories_fail_without_touching_worktree_or_ignored_files() {
    for strategy in [None, Some("--rebase"), Some("--merge")] {
        for has_ignored_file in [true, false] {
            let r = Repo::new();
            let tree = r.create("topic");
            r.git(&tree, &["checkout", "--orphan", "orphan"]);
            r.git(&tree, &["rm", "-rf", "."]);
            r.write_commit(&tree, ".gitignore", "valuable\n");
            fs::write(tree.join("valuable"), "temporary history contents\n").unwrap();
            r.git(&tree, &["add", "-f", "valuable"]);
            r.git(&tree, &["commit", "-m", "temporary path"]);
            r.git(&tree, &["rm", "valuable"]);
            r.git(&tree, &["commit", "-m", "remove temporary path"]);
            r.write_commit(&tree, "own", "unrelated work\n");
            if has_ignored_file {
                fs::write(tree.join("valuable"), b"precious\0ignored data\n").unwrap();
            }
            let merge_base = r
                .command("git", &tree, &["merge-base", "HEAD", "origin/main"])
                .output()
                .unwrap();
            assert_eq!(merge_base.status.code(), Some(1));
            assert!(merge_base.stdout.is_empty());
            assert!(merge_base.stderr.is_empty());
            let before = r.snapshot(&tree);
            let refs = r.git(&tree, &["show-ref"]);
            let mut args = vec![];
            args.extend(strategy);
            let doc = r.sync(&tree, &args, 1);
            let item = &doc["data"]["items"][0];
            assert_eq!(item["status"], "failed", "{doc}");
            assert!(
                item["detail"]
                    .as_str()
                    .unwrap()
                    .contains("no common history with refs/remotes/origin/main"),
                "{doc}"
            );
            assert_eq!(r.snapshot(&tree), before);
            assert_eq!(r.git(&tree, &["show-ref"]), refs);
            assert_eq!(item["before"], item["after"]);
            if has_ignored_file {
                assert_eq!(
                    fs::read(tree.join("valuable")).unwrap(),
                    b"precious\0ignored data\n"
                );
            }
        }
    }
}

#[test]
fn ignored_paths_touched_by_intermediate_commits_are_preserved() {
    for side in ["local", "base"] {
        for shape in ["delete", "rename", "copy", "directory", "file"] {
            for strategy in ["rebase", "merge"] {
                if side == "local" && strategy == "merge" {
                    continue;
                }
                let r = Repo::new();
                let tree = r.create("topic");
                let history = if side == "local" { &tree } else { &r.upstream };
                let touched = if shape == "directory" {
                    "cache/child"
                } else {
                    "cache"
                };
                if shape == "directory" {
                    fs::create_dir(history.join("cache")).unwrap();
                }
                r.write_commit(history, touched, "temporary\n");
                if shape == "rename" {
                    r.git(history, &["mv", touched, "renamed"]);
                } else if shape == "copy" {
                    fs::copy(history.join(touched), history.join("copied")).unwrap();
                    r.git(history, &["add", "copied"]);
                    r.git(history, &["rm", touched]);
                } else {
                    r.git(history, &["rm", touched]);
                }
                r.git(history, &["commit", "-m", "remove temporary path"]);
                // Keep a real own diff so landed detection does not skip integration.
                r.write_commit(&tree, "own", "own\n");
                r.advance();
                let ignore = r.root.join("ignore");
                fs::write(&ignore, "cache\n").unwrap();
                r.git(
                    &tree,
                    &["config", "core.excludesFile", ignore.to_str().unwrap()],
                );
                let protected = if shape == "file" {
                    fs::create_dir(tree.join("cache")).unwrap();
                    "cache/private"
                } else {
                    "cache"
                };
                fs::write(tree.join(protected), "user data").unwrap();
                let before = r.git(&tree, &["rev-parse", "HEAD"]);
                let flag = format!("--{strategy}");
                assert_eq!(
                    r.sync(&tree, &[&flag], 0)["data"]["items"][0]["status"],
                    "dirty",
                    "{side} {shape} {strategy}"
                );
                assert_eq!(
                    fs::read_to_string(tree.join(protected)).unwrap(),
                    "user data"
                );
                assert_eq!(r.git(&tree, &["rev-parse", "HEAD"]), before);
            }
        }
    }
}

#[test]
fn merge_ignores_user_options_and_signing_configuration() {
    for options in [
        "--no-commit",
        "--squash",
        "--ff-only",
        "--verify-signatures --gpg-sign",
    ] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "own\n");
        let before = r.git(&tree, &["rev-parse", "HEAD"]);
        r.git(&tree, &["push", "origin", "topic"]);
        r.advance();
        r.git(&tree, &["config", "branch.topic.mergeOptions", options]);
        r.git(&tree, &["config", "merge.ff", "only"]);
        r.git(&tree, &["config", "merge.verifySignatures", "true"]);
        r.git(&tree, &["config", "merge.autoStash", "true"]);
        r.git(&tree, &["config", "commit.gpgSign", "true"]);
        r.git(&tree, &["config", "gpg.program", "/does/not/exist"]);
        let doc = r.sync(&tree, &[], 0);
        assert_eq!(doc["data"]["items"][0]["status"], "updated", "{options}");
        assert_eq!(doc["data"]["items"][0]["strategy"], "merge");
        assert_eq!(r.git(&tree, &["rev-parse", "HEAD^1"]), before);
        assert_eq!(
            r.git(&tree, &["rev-parse", "HEAD^2"]),
            r.git(&tree, &["rev-parse", "origin/main"])
        );
        assert_eq!(r.git(&tree, &["status", "--porcelain=v2"]), "");
        assert!(!tree
            .join(r.git(&tree, &["rev-parse", "--git-path", "MERGE_HEAD"]))
            .exists());
        assert_eq!(r.git(&tree, &["stash", "list"]), "");
    }
}

#[test]
fn rebase_preserves_fixup_commits_and_disables_user_rebase_configuration() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    fs::write(tree.join("own"), "fixed\n").unwrap();
    r.git(&tree, &["add", "own"]);
    r.git(&tree, &["commit", "--fixup", "HEAD"]);
    let subjects = r.git(&tree, &["log", "--format=%s", "origin/main..HEAD"]);
    r.git(&tree, &["branch", "other-ref"]);
    let other = r.git(&tree, &["rev-parse", "other-ref"]);
    for key in [
        "rebase.autoSquash",
        "rebase.rebaseMerges",
        "rebase.updateRefs",
        "rebase.autoStash",
        "commit.gpgSign",
    ] {
        r.git(&tree, &["config", key, "true"]);
    }
    r.git(&tree, &["config", "gpg.program", "/does/not/exist"]);
    r.advance();
    let doc = r.sync(&tree, &[], 0);
    assert_eq!(doc["data"]["items"][0]["status"], "updated");
    assert_eq!(
        r.git(&tree, &["log", "--format=%s", "origin/main..HEAD"]),
        subjects
    );
    assert_eq!(r.git(&tree, &["rev-parse", "other-ref"]), other);
    assert_eq!(r.git(&tree, &["status", "--porcelain=v2"]), "");
    assert_eq!(r.git(&tree, &["stash", "list"]), "");
}

#[test]
fn successful_but_incomplete_integration_is_failed_and_restored() {
    use std::os::unix::fs::PermissionsExt;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|file| file.is_file())
        .unwrap();
    for mode in ["merge", "squash", "rebase"] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "own\n");
        fs::write(tree.join(".gitignore"), "ignored\n").unwrap();
        r.git(&tree, &["add", ".gitignore"]);
        r.git(&tree, &["commit", "-m", "ignore"]);
        fs::write(tree.join("ignored"), b"precious\0bytes").unwrap();
        r.advance();
        let before = r.snapshot(&tree);
        let fsmonitor = r.install_fsmonitor();
        let bin = r.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        // Simulate a Git process returning success after leaving real operation
        // state or a squash index behind, despite our explicit command options.
        fs::write(
            &wrapper,
            format!(
                r#"#!/bin/sh
for arg do
    if [ "$arg" = --abort ]; then exec "$REAL_GIT" "$@"; fi
done
for arg do
    case "$arg" in
        merge|rebase)
            case '{mode}' in
                merge) "$REAL_GIT" -c core.hooksPath=/dev/null -c core.fsmonitor=false merge --no-commit origin/main ;;
                squash) "$REAL_GIT" -c core.hooksPath=/dev/null -c core.fsmonitor=false merge --squash origin/main ;;
                rebase) "$REAL_GIT" -c core.hooksPath=/dev/null -c core.fsmonitor=false rebase --exec false origin/main ;;
            esac
            exit 0 ;;
    esac
done
exec "$REAL_GIT" "$@"
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let out = r
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &tree,
                &[
                    "sync",
                    "--json",
                    "--keep-conflicts",
                    if mode == "rebase" {
                        "--rebase"
                    } else {
                        "--merge"
                    },
                ],
            )
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("REAL_GIT", &real_git)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["data"]["items"][0]["status"], "failed");
        assert!(
            !fsmonitor.exists(),
            "fsmonitor ran during incomplete integration rollback"
        );
        r.git(&r.main, &["config", "core.fsmonitor", "false"]);
        assert_eq!(r.snapshot(&tree), before, "{mode}");
        for state in ["MERGE_HEAD", "rebase-merge", "rebase-apply"] {
            assert!(!tree
                .join(r.git(&tree, &["rev-parse", "--git-path", state]))
                .exists());
        }
    }
}

#[test]
fn rebase_does_not_preserve_merges_from_user_config() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.git(&tree, &["checkout", "-b", "side"]);
    r.write_commit(&tree, "side", "side\n");
    r.git(&tree, &["checkout", "topic"]);
    r.write_commit(&tree, "other", "other\n");
    r.git(&tree, &["merge", "--no-ff", "--no-edit", "side"]);
    r.git(&tree, &["config", "rebase.rebaseMerges", "true"]);
    r.advance();
    assert_eq!(
        r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
        "updated"
    );
    for file in ["own", "side", "other"] {
        assert_eq!(
            fs::read_to_string(tree.join(file)).unwrap(),
            format!("{file}\n")
        );
    }
    assert_eq!(
        r.git(&tree, &["rev-list", "--merges", "origin/main..HEAD"]),
        ""
    );
    assert_eq!(
        r.git(&tree, &["rev-list", "--count", "origin/main..HEAD"]),
        "3"
    );
}

#[test]
fn slash_remote_default_and_explicit_bases_fetch_and_integrate_main() {
    for overlapping in [false, true] {
        let r = Repo::new();
        r.git(&r.main, &["remote", "rename", "origin", "team/origin"]);
        if overlapping {
            // Fetching the shorter prefix would fail, and must never be attempted.
            // Git >= 2.51 rejects overlapping names in `remote add`; config can
            // still contain them, including repositories made with older Git.
            r.git(&r.main, &["config", "remote.team.url", "/does/not/exist"]);
            r.git(
                &r.main,
                &[
                    "config",
                    "remote.team.fetch",
                    "+refs/heads/*:refs/remotes/team/*",
                ],
            );
        }
        for base in [
            None,
            Some("team/origin/main"),
            Some("refs/remotes/team/origin/main"),
        ] {
            let name = format!("topic-{}", base.map_or(0, str::len));
            let mut args = vec!["create", &name, "--json"];
            if let Some(base) = base {
                args.extend(["--base", base]);
            }
            let out = r.rabi(&r.main, &args);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
            let tree = PathBuf::from(doc["data"]["workspace"]["path"].as_str().unwrap());
            let out = r.rabi(&r.main, &["set", &name, "--base", "team/origin/main"]);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            r.write_commit(&r.upstream, &name, "incoming\n");
            r.git(&r.upstream, &["push", "origin", "main"]);
            let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
            let doc = r.sync(&r.main, &[], 0);
            assert_eq!(doc["data"]["main"]["name"], "main");
            assert_eq!(doc["data"]["main"]["status"], "updated");
            assert_eq!(r.git(&r.main, &["rev-parse", "main"]), expected);
            assert_eq!(r.git(&r.main, &["branch", "--list", "origin/main"]), "");
            r.write_commit(&r.upstream, &format!("{name}-next"), "next\n");
            r.git(&r.upstream, &["push", "origin", "main"]);
            let expected = r.git(&r.upstream, &["rev-parse", "HEAD"]);
            let doc = r.sync(&tree, &[], 0);
            assert_eq!(
                doc["data"]["items"][0]["base"],
                "refs/remotes/team/origin/main"
            );
            assert_eq!(doc["data"]["items"][0]["status"], "updated");
            assert_eq!(r.git(&tree, &["rev-parse", "HEAD"]), expected);
        }
    }
}

#[test]
fn unmatched_default_remote_fails_without_creating_a_branch() {
    // Design §4: request primary explicitly when verifying both scopes.
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    r.git(&r.main, &["update-ref", "refs/remotes/stale/main", "HEAD"]);
    r.git(
        &r.main,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/stale/main",
        ],
    );
    let before = r.git(&r.main, &["for-each-ref", "refs/heads"]);
    let doc = r.sync(&r.main, &[], 1);
    assert_eq!(doc["data"]["main"]["status"], "failed");
    assert!(doc["data"]["main"]["detail"]
        .as_str()
        .unwrap()
        .contains("No configured remote matches"));
    assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), before);
    r.advance();
    let doc = r.sync(&r.main, &["topic", "--main"], 6);
    assert_eq!(doc["data"]["main"]["status"], "failed");
    assert_eq!(doc["data"]["items"][0]["status"], "updated", "{doc}");
    assert!(tree.join("remote-file").exists());
}

fn pause_root_rebase(r: &Repo, path: &Path, update_refs: bool) {
    let editor = r.root.join("sequence-editor");
    fs::write(&editor, "#!/bin/sh\nsed -i '1s/^pick /edit /' \"$1\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).unwrap();
    let mut cmd = r.command("git", path, &["rebase", "-i", "--root"]);
    cmd.env("GIT_SEQUENCE_EDITOR", &editor);
    if update_refs {
        cmd.arg("--update-refs");
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn default_reserved_by_rebase_in_main_or_linked_checkout() {
    for linked in [false, true] {
        let r = Repo::new();
        let path = if linked {
            r.git(&r.main, &["switch", "-c", "other"]);
            let path = r.root.join("default-tree");
            r.git(
                &r.main,
                &["worktree", "add", path.to_str().unwrap(), "main"],
            );
            path
        } else {
            r.main.clone()
        };
        let before = r.git(&r.main, &["rev-parse", "main"]);
        pause_root_rebase(&r, &path, false);
        let topic = r.create("topic");
        r.advance();
        let doc = r.sync(&r.main, &[], 0);
        assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
        // Design §3.4: detached/non-default primary skips before integration.
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
        let doc = r.sync(&r.main, &["topic", "--main"], 0);
        assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
        assert_eq!(doc["data"]["items"][0]["status"], "updated");
        assert!(topic.exists());
        r.git(&path, &["rebase", "--continue"]);
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
    }
}

#[test]
fn default_reserved_by_update_refs_and_bisect() {
    for operation in ["update-refs", "bisect"] {
        let r = Repo::new();
        r.git(&r.main, &["switch", "-c", "other"]);
        if operation == "update-refs" {
            // main points at an intermediate commit, so --update-refs reserves it.
            r.write_commit(&r.main, "other-file", "other\n");
            pause_root_rebase(&r, &r.main, true);
        } else {
            r.write_commit(&r.main, "other-file", "other\n");
            r.git(&r.main, &["switch", "main"]);
            r.git(&r.main, &["bisect", "start", "other", "main"]);
        }
        let before = r.git(&r.main, &["rev-parse", "main"]);
        r.advance();
        let doc = r.sync(&r.main, &[], if operation == "bisect" { 1 } else { 0 });
        assert_eq!(
            doc["data"]["main"]["status"],
            if operation == "bisect" {
                "dirty"
            } else {
                "not-default-branch"
            },
            "{doc}"
        );
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
        if operation == "update-refs" {
            r.git(&r.main, &["rebase", "--continue"]);
        } else {
            r.git(&r.main, &["bisect", "reset"]);
        }
    }
}

#[test]
fn target_operations_are_never_integrated() {
    for operation in ["rebase", "merge", "cherry-pick", "revert", "bisect"] {
        let r = Repo::new();
        let tree = r.create("topic");
        if operation == "rebase" {
            pause_root_rebase(&r, &tree, false);
        } else {
            // Resolved operations can have a clean index while their state remains.
            let state = match operation {
                "merge" => "MERGE_HEAD",
                "cherry-pick" => "CHERRY_PICK_HEAD",
                "revert" => "REVERT_HEAD",
                _ => "BISECT_START",
            };
            let file = r.git(&tree, &["rev-parse", "--git-path", state]);
            fs::write(tree.join(file), r.git(&tree, &["rev-parse", "HEAD"])).unwrap();
        }
        r.advance();
        let before = r.snapshot(&tree);
        let doc = r.sync(&r.main, &["topic"], 0);
        assert_eq!(
            doc["data"]["items"][0]["status"],
            if operation == "rebase" {
                "detached"
            } else {
                "dirty"
            }
        );
        assert!(doc["data"]["items"][0]["detail"]
            .as_str()
            .unwrap()
            .contains(&format!("{operation} in progress")));
        assert_eq!(r.snapshot(&tree), before);
    }
}

#[test]
fn target_reserved_by_another_worktrees_update_refs_is_dirty() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.git(&tree, &["switch", "--detach"]);
    r.git(&r.main, &["switch", "-c", "other"]);
    r.write_commit(&r.main, "other-file", "other\n");
    pause_root_rebase(&r, &r.main, true);
    // Reattaching after the reservation makes the target eligible by HEAD alone.
    r.git(&tree, &["switch", "topic"]);
    r.advance();
    let before = r.snapshot(&tree);
    let doc = r.sync(&r.main, &["topic"], 0);
    assert_eq!(doc["data"]["items"][0]["status"], "dirty");
    assert_eq!(r.snapshot(&tree), before);
    r.git(&r.main, &["rebase", "--continue"]);
}

#[test]
fn prunable_worktree_is_skipped_without_blocking_live_sync() {
    // Design §4: request primary explicitly when verifying both scopes.
    let r = Repo::new();
    let gone = r.create("gone");
    let before = r.git(&gone, &["rev-parse", "HEAD"]);
    let admin = PathBuf::from(r.git(&gone, &["rev-parse", "--absolute-git-dir"]));
    let registration = file_contents(&admin);
    fs::remove_dir_all(&gone).unwrap();
    let live = r.create("live");
    r.advance();
    let doc = r.sync(&r.main, &["--all", "--main"], 0);
    assert_eq!(doc["data"]["main"]["status"], "updated");
    let items = doc["data"]["items"].as_array().unwrap();
    assert_eq!(
        items.iter().find(|v| v["name"] == "gone").unwrap()["status"],
        "dirty"
    );
    assert_eq!(
        items.iter().find(|v| v["name"] == "live").unwrap()["status"],
        "updated"
    );
    assert_eq!(
        r.git(&live, &["rev-parse", "HEAD"]),
        r.git(&r.upstream, &["rev-parse", "HEAD"])
    );
    assert_eq!(r.git(&r.main, &["rev-parse", "gone"]), before);
    assert_eq!(file_contents(&admin), registration);
    assert!(!gone.exists());
}

#[test]
fn missing_linked_checkout_still_reserves_default_during_rebase() {
    let r = Repo::new();
    r.git(&r.main, &["switch", "-c", "other"]);
    let gone = r.root.join("reserved-main");
    r.git(
        &r.main,
        &["worktree", "add", gone.to_str().unwrap(), "main"],
    );
    pause_root_rebase(&r, &gone, false);
    let admin = PathBuf::from(r.git(&gone, &["rev-parse", "--absolute-git-dir"]));
    let state = file_contents(&admin);
    let before = r.git(&r.main, &["rev-parse", "main"]);
    fs::remove_dir_all(&gone).unwrap();
    let live = r.create("live");
    r.advance();
    let doc = r.sync(&r.main, &["live", "--main"], 0);
    assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
    // Design §3.4: non-default primary skips without touching reserved main.
    assert_eq!(doc["data"]["items"][0]["status"], "updated");
    assert!(live.is_dir());
    assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
    assert_eq!(file_contents(&admin), state);
}

#[test]
fn missing_default_checkout_and_broken_target_do_not_block_live_target() {
    // Design §4: request primary explicitly when verifying both scopes.
    let r = Repo::new();
    let bad = r.create("bad");
    let live = r.create("live");
    // Keep a directory but break its Git registration: only this item fails.
    fs::write(bad.join(".git"), "gitdir: /nonexistent/rabi-test\n").unwrap();
    r.git(&r.main, &["switch", "-c", "other"]);
    let missing = r.root.join("missing-main");
    r.git(
        &r.main,
        &["worktree", "add", missing.to_str().unwrap(), "main"],
    );
    fs::remove_dir_all(&missing).unwrap();
    let before = r.git(&r.main, &["rev-parse", "main"]);
    r.advance();
    let doc = r.sync(&r.main, &["--all", "--main"], 6);
    assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
    let items = doc["data"]["items"].as_array().unwrap();
    assert_eq!(
        items.iter().find(|v| v["name"] == "bad").unwrap()["status"],
        "failed"
    );
    assert_eq!(
        items.iter().find(|v| v["name"] == "live").unwrap()["status"],
        "updated"
    );
    assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
    assert_eq!(
        r.git(&live, &["rev-parse", "HEAD"]),
        r.git(&r.upstream, &["rev-parse", "HEAD"])
    );
}

#[test]
fn context_errors_use_shared_exit_codes() {
    let r = Repo::new();
    for args in [
        vec!["sync", "--repo", "missing-project", "--json"],
        vec!["list", "--repo", "missing-project", "--json"],
    ] {
        assert_eq!(r.rabi(&r.main, &args).status.code(), Some(3));
    }
    assert_eq!(r.rabi(&r.root, &["sync", "--json"]).status.code(), Some(3));
}

#[test]
fn metadata_failure_after_fast_forward_restores_worktree() {
    use std::os::unix::fs::PermissionsExt;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|file| file.is_file())
        .unwrap();
    for fail_rollback in ["0", "1", "silent"] {
        let r = Repo::new();
        let tree = r.create("topic");
        let store = r.main.join(".git/rabi/workspaces");
        let original_store = records::read(&store).unwrap();
        r.advance();
        let before = r.snapshot(&tree);
        let fsmonitor = r.install_fsmonitor();
        let bin = r.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
for arg do
    if [ "$arg" = --keep ] && [ "$FAIL_ROLLBACK" = 1 ]; then
        echo "injected rollback failure" >&2
        exit 1
    fi
    if [ "$arg" = --keep ] && [ "$FAIL_ROLLBACK" = silent ]; then
        exit 1
    fi
done
for arg do
    if [ "$arg" = merge ]; then
        "$REAL_GIT" "$@" || exit $?
        for record in "$STORE"/*.json; do printf 'invalid json' > "$record"; done
        exit 0
    fi
done
exec "$REAL_GIT" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let out = r
            .command(env!("CARGO_BIN_EXE_rabi"), &tree, &["sync", "--json"])
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("REAL_GIT", &real_git)
            .env("STORE", &store)
            .env("FAIL_ROLLBACK", fail_rollback)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(if fail_rollback == "0" { 1 } else { 6 }),
            "{out:?}"
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        let item = &doc["data"]["items"][0];
        assert_eq!(item["status"], "failed");
        assert!(
            !fsmonitor.exists(),
            "fsmonitor ran during metadata rollback"
        );
        r.git(&r.main, &["config", "core.fsmonitor", "false"]);
        let detail = item["detail"].as_str().unwrap();
        assert!(detail.contains("metadata update failed"), "{detail}");
        if fail_rollback != "0" {
            assert!(detail.contains("rollback failed"), "{detail}");
        } else {
            assert!(detail.contains("workspace restored"), "{detail}");
            assert_eq!(item["before"], item["after"]);
            assert_eq!(r.snapshot(&tree), before);
            records::write(&store, &original_store).unwrap();
            assert_eq!(
                r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
                "updated"
            );
            assert_eq!(
                r.sync(&tree, &[], 0)["data"]["items"][0]["status"],
                "up-to-date"
            );
            let out = r.rabi(&r.main, &["gc", "--json"]);
            assert!(!String::from_utf8_lossy(&out.stdout).contains("topic"));
        }
    }
}

#[test]
fn default_branch_merge_options_cannot_autostash_local_edits() {
    let r = Repo::new();
    r.git(
        &r.main,
        &["config", "branch.main.mergeOptions", "--autostash"],
    );
    r.write_commit(&r.upstream, "file", "incoming edit\n");
    r.git(&r.upstream, &["push", "origin", "main"]);
    fs::write(r.main.join("file"), "local edit\n").unwrap();
    let before = r.snapshot(&r.main);
    let stash = r.git(&r.main, &["stash", "list"]);
    let out = r.rabi(&r.main, &["sync", "--json"]);
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["main"]["status"], "dirty", "{doc}");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r.snapshot(&r.main), before);
    assert_eq!(r.git(&r.main, &["stash", "list"]), stash);
    assert!(!r.main.join(".git/MERGE_AUTOSTASH").exists());
}

#[test]
fn failed_default_remote_does_not_block_a_healthy_base_remote() {
    // Design §4: request primary explicitly when verifying both scopes.
    let r = Repo::new();
    r.git(
        &r.main,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/cache/*",
        ],
    );
    r.git(&r.main, &["fetch", "origin"]);
    r.git(
        &r.main,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/cache/main",
        ],
    );
    let healthy = r.create("healthy");
    let stale = r.create("stale");
    r.write_commit(&healthy, "own", "own\n");
    r.write_commit(&stale, "local", "local\n");
    r.git(
        &r.main,
        &[
            "remote",
            "add",
            "healthy",
            r.root.join("remote.git").to_str().unwrap(),
        ],
    );
    r.git(&r.main, &["fetch", "healthy"]);
    // Git >= 2.48 creates this on fetch; exercise the same state on older Git.
    r.git(&r.main, &["remote", "set-head", "healthy", "main"]);
    assert!(r
        .rabi(&r.main, &["set", "healthy", "--base", "healthy/main"])
        .status
        .success());
    r.advance();
    r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
    let default_before = r.snapshot(&r.main);
    let stale_before = r.snapshot(&stale);
    let doc = r.sync(&r.main, &["healthy", "stale", "--main"], 6);
    assert_eq!(
        r.git(&r.main, &["symbolic-ref", "refs/remotes/healthy/HEAD"]),
        "refs/remotes/healthy/main"
    );
    assert_eq!(doc["data"]["main"]["status"], "failed");
    assert_eq!(doc["data"]["items"][0]["status"], "updated", "{doc}");
    assert_eq!(doc["data"]["items"][1]["status"], "failed");
    assert_eq!(doc["data"]["items"][1]["base"], "refs/remotes/cache/main");
    for item in [&doc["data"]["main"], &doc["data"]["items"][1]] {
        assert!(item["detail"]
            .as_str()
            .unwrap()
            .contains("Fetch origin failed"));
    }
    assert!(healthy.join("remote-file").exists());
    assert_eq!(r.snapshot(&r.main), default_before);
    assert_eq!(r.snapshot(&stale), stale_before);
}

#[test]
fn fetch_failures_exit_one_even_with_a_published_rebase_refusal() {
    let r = Repo::new();
    let tree = r.create("published");
    let stale = r.create("stale");
    r.write_commit(&tree, "own", "own\n");
    r.git(
        &r.main,
        &[
            "remote",
            "add",
            "healthy",
            r.root.join("remote.git").to_str().unwrap(),
        ],
    );
    r.git(&tree, &["push", "healthy", "published"]);
    r.git(&r.main, &["fetch", "healthy"]);
    assert!(r
        .rabi(&r.main, &["set", "published", "--base", "healthy/main"])
        .status
        .success());
    r.advance();
    r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
    let before = r.snapshot(&tree);
    for args in [
        vec!["published", "--rebase", "--main"],
        vec!["published", "stale", "--rebase"],
        vec!["stale", "published", "--rebase"],
    ] {
        let doc = r.sync(&r.main, &args, 1);
        assert!(doc["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["code"] == "published"));
        assert_eq!(r.snapshot(&tree), before);
        assert!(!stale.join("remote-file").exists());
    }
}

#[test]
fn git_hook_read_only_intermediary_can_run_nested_rabi() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    let r = Repo::new();
    r.create("existing");
    let bin = env!("CARGO_BIN_EXE_rabi");
    let marker = r.root.join("monitor-entered");
    let nested_status = r.root.join("monitor-status");
    let monitor = r.root.join("monitor");
    fs::write(
        &monitor,
        format!(
            r#"#!/bin/sh
if [ ! -e '{marker}' ]; then
    touch '{marker}'
    '{bin}' gc --json > '{output}' 2>&1
    echo $? > '{status}'
fi
printf 'token\000/\000'
"#,
            marker = marker.display(),
            output = r.root.join("nested-output").display(),
            status = nested_status.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&monitor, fs::Permissions::from_mode(0o755)).unwrap();
    let hook = r.main.join(".git/hooks/post-checkout");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\ncd '{}'\ngit config core.fsmonitor '{}'\n'{bin}' list --json\n",
            r.main.display(),
            monitor.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = r
        .command(bin, &r.main, &["create", "outer", "--no-setup"])
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() {
            break false;
        }
        if std::time::Instant::now() >= deadline {
            Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .status()
                .unwrap();
            break true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let out = child.wait_with_output().unwrap();
    assert!(
        !timed_out,
        "read-only Git intermediary deadlocked: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(nested_status).unwrap().trim(), "0");
}

#[test]
fn gc_protects_linked_default_with_a_remapped_tracking_namespace() {
    for (refspec, destination) in [
        (
            "+refs/heads/*:refs/remotes/cache/*",
            "refs/remotes/cache/main",
        ),
        ("+refs/heads/*:refs/cache/*", "refs/cache/main"),
        ("+refs/heads/main:refs/cache/main", "refs/cache/main"),
        ("+refs/heads/m*in:refs/cache/m*in", "refs/cache/main"),
    ] {
        let r = Repo::new();
        r.git(&r.main, &["config", "remote.origin.fetch", refspec]);
        r.git(&r.main, &["fetch", "origin"]);
        r.git(
            &r.main,
            &["symbolic-ref", "refs/remotes/origin/HEAD", destination],
        );
        let landed = r.create("landed");
        r.write_commit(&landed, "landed", "landed change\n");
        r.git(&r.main, &["merge", "--ff-only", "landed"]);
        r.git(&r.main, &["push", "origin", "main"]);
        r.git(&r.upstream, &["pull", "--ff-only"]);
        r.git(&r.main, &["checkout", "-b", "other"]);
        assert!(r
            .rabi(&r.main, &["create", "--branch", "main"])
            .status
            .success());
        // main has moved since creation, so only the default-branch guard protects it.
        r.advance();
        let doc = r.sync(&r.main, &[], 0);
        let tree = PathBuf::from(doc["data"]["main"]["path"].as_str().unwrap());
        let before = r.snapshot(&tree);
        let out = r.rabi(&r.main, &["gc", "--merged", "--apply", "--json"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(tree.exists(), "primary checkout removed: {doc}");
        assert_eq!(r.snapshot(&tree), before);
        assert!(
            !doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["branch"] == "main" && item["action"] == "removed"),
            "{doc}"
        );
        assert!(
            doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["branch"] == "landed" && item["action"] == "removed"),
            "landed branch was not selected: {doc}"
        );
        assert!(!landed.exists());
    }
}

#[test]
fn gc_local_branch_named_like_a_remapped_default_is_not_the_default() {
    for ambiguous_refs in ["true", "false"] {
        let r = Repo::new();
        r.git(
            &r.main,
            &["config", "core.warnAmbiguousRefs", ambiguous_refs],
        );
        r.git(
            &r.main,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/cache/*",
            ],
        );
        r.git(&r.main, &["fetch", "origin"]);
        r.git(
            &r.main,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/cache/main",
            ],
        );
        let collision = r.create("cache/main");
        r.write_commit(&collision, "landed", "landed change\n");
        r.git(&r.main, &["merge", "--ff-only", "cache/main"]);
        r.git(&r.main, &["push", "origin", "main"]);
        r.git(&r.main, &["fetch", "origin"]);
        r.git(&r.main, &["checkout", "-b", "other"]);
        let out = r.rabi(&r.main, &["create", "--branch", "main", "--json"]);
        assert!(out.status.success());
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        let default_path = PathBuf::from(doc["data"]["workspace"]["path"].as_str().unwrap());
        let before = r.snapshot(&default_path);
        let out = r.rabi(&r.main, &["gc", "--merged", "--json"]);
        assert!(out.status.success());
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(
            doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["branch"] == "cache/main" && item["action"] == "wouldRemove"),
            "local cache/main was mistaken for the default: {doc}"
        );
        assert!(
            !doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["branch"] == "main" && item["action"] == "wouldRemove"),
            "default main was selected: {doc}"
        );
        assert_eq!(r.snapshot(&default_path), before);
        let out = r.rabi(&r.main, &["gc", "--merged", "--apply", "--json"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(
            doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["branch"] == "cache/main" && item["action"] == "removed"),
            "{doc}"
        );
        assert!(!collision.exists());
        assert!(default_path.exists(), "{doc}");
        assert_eq!(r.snapshot(&default_path), before);
    }
}

#[test]
fn fetch_refspec_namespace_determines_remote_and_default_local_name() {
    // Design §4: request primary explicitly when verifying both scopes.
    for (refspec, upstream, remote_head) in [
        ("+refs/heads/*:refs/remotes/cache/*", false, true),
        ("+refs/heads/*:refs/remotes/cache/*", true, false),
        ("+refs/heads/main:refs/remotes/cache/main", false, false),
        ("+refs/heads/m*in:refs/remotes/cache/m*in", true, true),
    ] {
        let r = Repo::new();
        r.git(&r.main, &["config", "remote.origin.fetch", refspec]);
        r.git(&r.main, &["fetch", "origin"]);
        r.git(
            &r.main,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/cache/main",
            ],
        );
        if !remote_head {
            r.git(&r.main, &["remote", "set-head", "origin", "--delete"]);
        }
        if upstream {
            r.git(&r.main, &["config", "branch.main.remote", "origin"]);
            r.git(&r.main, &["config", "branch.main.merge", "refs/heads/main"]);
        }
        let tree = r.create("topic");
        let out = r.rabi(
            &r.main,
            &["set", "topic", "--base", "refs/remotes/cache/main"],
        );
        assert!(out.status.success());
        r.write_commit(&tree, "local", "local\n");
        r.advance();
        let tip = r.git(&r.upstream, &["rev-parse", "HEAD"]);
        let doc = r.sync(&r.main, &["topic", "--main"], 0);
        assert_eq!(doc["data"]["main"]["status"], "updated", "{doc}");
        assert_eq!(doc["data"]["main"]["branch"], "refs/heads/main");
        assert_eq!(doc["data"]["items"][0]["status"], "updated", "{doc}");
        assert_eq!(r.git(&r.main, &["rev-parse", "main"]), tip);
        assert_eq!(
            r.git(&r.main, &["rev-parse", "refs/remotes/cache/main"]),
            tip
        );
        assert!(tree.join("remote-file").exists());
        assert_eq!(r.git(&r.main, &["branch", "--list", "cache/main"]), "");
    }
}

#[test]
fn unresolved_default_does_not_block_recorded_bases() {
    // Design §4: request primary explicitly when verifying both scopes.
    let r = Repo::new();
    r.git(&r.upstream, &["branch", "-m", "develop"]);
    r.git(&r.upstream, &["push", "origin", "develop"]);
    let clone = r.root.join("single-branch");
    r.git(
        &r.main,
        &[
            "clone",
            "--single-branch",
            "--branch",
            "develop",
            r.root.join("remote.git").to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    r.git(&clone, &["config", "user.name", "Sync Test"]);
    r.git(&clone, &["config", "user.email", "sync@example.com"]);
    r.git(&clone, &["remote", "set-head", "origin", "--delete"]);
    let out = r.rabi(
        &clone,
        &["create", "topic", "--base", "origin/develop", "--json"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let created: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tree = PathBuf::from(created["data"]["workspace"]["path"].as_str().unwrap());
    r.write_commit(&tree, "own", "own\n");
    // Records without an explicit unknown marker exercise default-base errors.
    r.git(&clone, &["branch", "sparse"]);
    assert!(r
        .rabi(&clone, &["create", "--branch", "sparse"])
        .status
        .success());
    let store = clone.join(".git/rabi/workspaces");
    let mut sparse: Value = serde_json::from_slice(&records::read(&store).unwrap()).unwrap();
    let record = sparse["worktrees"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["name"] == "sparse")
        .unwrap();
    assert_eq!(record["baseCleared"], true);
    record.as_object_mut().unwrap().remove("baseCleared");
    records::write(&store, serde_json::to_vec(&sparse).unwrap()).unwrap();
    r.write_commit(&r.upstream, "incoming-develop", "incoming\n");
    r.git(&r.upstream, &["push", "origin", "develop"]);
    let doc = r.sync(&clone, &["sparse", "topic", "--main"], 6);
    assert_eq!(doc["data"]["main"]["status"], "failed", "{doc}");
    assert_eq!(doc["data"]["items"][0]["status"], "failed", "{doc}");
    for item in [&doc["data"]["main"], &doc["data"]["items"][0]] {
        assert!(
            item["detail"]
                .as_str()
                .unwrap()
                .contains("Cannot determine the default branch"),
            "{doc}"
        );
    }
    assert_eq!(doc["data"]["items"][1]["status"], "updated", "{doc}");
    assert!(tree.join("incoming-develop").exists());
    let doc = r.sync(&clone, &["topic"], 0);
    assert!(doc["data"]["main"].is_null());
    assert_eq!(doc["data"]["items"][0]["status"], "up-to-date");
    let doc = r.sync(&clone, &["sparse", "topic"], 1);
    assert!(doc["data"]["main"].is_null());
    assert_eq!(doc["data"]["items"][0]["status"], "failed");
    assert_eq!(doc["data"]["items"][1]["status"], "up-to-date");
}

#[test]
fn ordinary_failures_take_precedence_over_pushed_refusals() {
    let r = Repo::new();
    let conflict = r.create("conflict");
    let published = r.create("published");
    r.write_commit(&conflict, "file", "conflicting local edit\n");
    r.write_commit(&published, "own", "published\n");
    r.git(&published, &["push", "origin", "published"]);
    r.write_commit(&r.upstream, "file", "conflicting remote edit\n");
    r.git(&r.upstream, &["push", "origin", "main"]);
    let before = r.snapshot(&conflict);
    for names in [["conflict", "published"], ["published", "conflict"]] {
        let doc = r.sync(&r.main, &[names[0], names[1], "--rebase"], 5);
        let items = doc["data"]["items"].as_array().unwrap();
        assert!(
            items.iter().any(|item| item["status"] == "conflict"),
            "{doc}"
        );
        assert!(
            items.iter().any(|item| item["code"] == "published"),
            "{doc}"
        );
        assert_eq!(r.snapshot(&conflict), before);
    }
    let doc = r.sync(&r.main, &["published", "--rebase"], 4);
    assert_eq!(doc["data"]["items"][0]["code"], "published");

    // A default failure is ordinary too, even alongside a sole refusal.
    r.git(&r.main, &["update-ref", "refs/remotes/stale/main", "HEAD"]);
    r.git(
        &r.main,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/stale/main",
        ],
    );
    let doc = r.sync(&r.main, &["published", "--rebase", "--main"], 1);
    assert_eq!(doc["data"]["main"]["status"], "failed");
    assert_eq!(doc["data"]["items"][0]["code"], "published");

    // A target inspection failure must also survive either ordering.
    fs::write(conflict.join(".git"), "gitdir: /does/not/exist\n").unwrap();
    for names in [["conflict", "published"], ["published", "conflict"]] {
        let doc = r.sync(&r.main, &[names[0], names[1], "--rebase"], 1);
        let items = doc["data"]["items"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|item| item["status"] == "failed" && item["code"].is_null()),
            "{doc}"
        );
        assert!(
            items.iter().any(|item| item["code"] == "published"),
            "{doc}"
        );
    }
}

#[test]
fn rebase_reapplies_upstream_cherry_pick_that_was_reverted() {
    for explicit in [false, true] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "local-change", "must survive\n");
        let local = r.git(&tree, &["rev-parse", "HEAD"]);
        r.advance();
        r.git(&r.upstream, &["fetch", r.main.to_str().unwrap(), "topic"]);
        r.git(&r.upstream, &["cherry-pick", &local]);
        r.git(&r.upstream, &["revert", "--no-edit", "HEAD"]);
        r.git(&r.upstream, &["push", "origin", "main"]);
        let args = if explicit { vec!["--rebase"] } else { vec![] };
        let doc = r.sync(&tree, &args, 0);
        assert_eq!(doc["data"]["items"][0]["strategy"], "rebase");
        assert_eq!(doc["data"]["items"][0]["status"], "updated");
        assert_eq!(
            fs::read_to_string(tree.join("local-change")).unwrap(),
            "must survive\n"
        );
    }
}

#[test]
fn rebase_preserves_intentionally_empty_and_newly_empty_commits() {
    for explicit in [false, true] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.git(
            &tree,
            &["commit", "--allow-empty", "-m", "intentional-empty"],
        );
        r.write_commit(&tree, "shared", "shared\n");
        r.write_commit(&tree, "own", "own\n");
        r.write_commit(&r.upstream, "shared", "shared\n");
        r.git(&r.upstream, &["push", "origin", "main"]);
        r.git(&tree, &["config", "rebase.backend", "apply"]);
        r.git(&tree, &["config", "rebase.missingCommitsCheck", "ignore"]);
        let args = if explicit { vec!["--rebase"] } else { vec![] };
        let doc = r.sync(&tree, &args, 0);
        assert_eq!(doc["data"]["items"][0]["status"], "updated");
        assert_eq!(
            r.git(&tree, &["rev-list", "--count", "origin/main..HEAD"]),
            "3"
        );
        assert!(r
            .git(&tree, &["log", "--format=%s", "origin/main..HEAD"])
            .contains("intentional-empty"));
    }
}

#[test]
fn merge_own_changes_refuse_rebase_and_survive_explicit_merge() {
    for resolution in [false, true] {
        for strategy in ["auto", "--rebase", "--merge"] {
            let r = Repo::new();
            let tree = r.create("topic");
            r.write_commit(&tree, "own", "own\n");
            r.git(&tree, &["checkout", "-b", "side"]);
            r.write_commit(&tree, if resolution { "file" } else { "side" }, "side\n");
            r.git(&tree, &["checkout", "topic"]);
            r.write_commit(&tree, if resolution { "file" } else { "other" }, "other\n");
            let out = r
                .command("git", &tree, &["merge", "--no-commit", "--no-ff", "side"])
                .output()
                .unwrap();
            assert_eq!(out.status.success(), !resolution);
            r.write_commit(
                &tree,
                if resolution { "file" } else { "merge-only" },
                "merge owns this\n",
            );
            let merge = r.git(&tree, &["rev-parse", "--short", "HEAD"]);
            r.advance();
            let before = r.snapshot(&tree);
            let mut args = vec![];
            if strategy != "auto" {
                args.push(strategy);
            }
            let doc = r.sync(&tree, &args, if strategy == "--merge" { 0 } else { 1 });
            let item = &doc["data"]["items"][0];
            if strategy == "--merge" {
                assert_eq!(item["status"], "updated");
                assert_eq!(item["strategy"], "merge");
                assert_eq!(
                    fs::read_to_string(tree.join(if resolution { "file" } else { "merge-only" }))
                        .unwrap(),
                    "merge owns this\n"
                );
            } else {
                assert_eq!(item["status"], "failed");
                assert_eq!(item["strategy"], "rebase");
                let detail = item["detail"].as_str().unwrap();
                assert!(
                    detail.contains(&merge)
                        && detail.contains("has changes of its own; use --merge"),
                    "{doc}"
                );
                assert_eq!(r.snapshot(&tree), before);
            }
        }
    }
}

#[test]
fn published_commits_in_independent_fetch_destinations_are_protected() {
    for (source, destination) in [
        ("refs/heads/*", "refs/heads/published/*"),
        ("refs/heads/topic", "refs/heads/published/topic"),
        ("refs/heads/to*ic", "refs/heads/published/to*ic"),
        ("refs/heads/*", "refs/tags/cache/*"),
        ("refs/heads/topic", "refs/tags/cache/topic"),
        ("refs/heads/to*ic", "refs/tags/cache/to*ic"),
        ("refs/heads/*", "refs/cache/*"),
    ] {
        for explicit in [false, true] {
            let r = Repo::new();
            let tree = r.create("topic");
            r.write_commit(&tree, "published", "published\n");
            r.git(&tree, &["push", "origin", "topic"]);
            // The base uses origin; archive is not fetched by sync, so its
            // independent destinations remain evidence even outside remotes/.
            r.git(
                &r.main,
                &[
                    "remote",
                    "add",
                    "archive",
                    r.root.join("remote.git").to_str().unwrap(),
                ],
            );
            r.git(
                &r.main,
                &[
                    "config",
                    "remote.archive.fetch",
                    &format!("+{source}:{destination}"),
                ],
            );
            r.git(&r.main, &["fetch", "archive"]);
            r.git(
                &r.main,
                &[
                    "config",
                    "remote.origin.fetch",
                    "+refs/heads/main:refs/remotes/origin/main",
                ],
            );
            r.git(&r.main, &["update-ref", "-d", "refs/remotes/origin/topic"]);
            r.advance();
            let before = r.snapshot(&tree);
            for dry_run in [true, false] {
                let mut args = Vec::new();
                if explicit {
                    args.push("--rebase");
                }
                if dry_run {
                    args.push("--dry-run");
                }
                let doc = r.sync(&tree, &args, if explicit { 4 } else { 0 });
                let item = &doc["data"]["items"][0];
                if explicit {
                    assert_eq!(item["code"], "published", "{doc}");
                    assert_eq!(r.snapshot(&tree), before);
                } else {
                    assert_eq!(item["strategy"], "merge", "{doc}");
                    assert_eq!(item["status"], "updated");
                    if dry_run {
                        assert!(item["after"].is_null());
                        assert_eq!(r.snapshot(&tree), before);
                    } else {
                        r.git(
                            &tree,
                            &[
                                "merge-base",
                                "--is-ancestor",
                                before["head"].as_str().unwrap(),
                                "HEAD",
                            ],
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn gc_default_uses_local_branch_when_a_same_name_tag_exists() {
    for annotated in [false, true] {
        let r = Repo::new();
        let gone = r.create("gone");
        r.git(&r.main, &["remote", "set-head", "origin", "--delete"]);
        if annotated {
            r.git(&r.main, &["tag", "-a", "main", "-m", "same-name tag"]);
        } else {
            r.git(&r.main, &["tag", "main"]);
        }
        r.write_commit(&r.main, "local-default", "keep default\n");
        r.git(&r.main, &["checkout", "-b", "other"]);
        assert!(r
            .rabi(&r.main, &["create", "--branch", "main"])
            .status
            .success());
        let default_path = PathBuf::from(
            r.git(&r.main, &["worktree", "list", "--porcelain"])
                .lines()
                .filter_map(|line| line.strip_prefix("worktree "))
                .find(|path| {
                    Path::new(path).join("local-default").exists() && Path::new(path) != r.main
                })
                .unwrap(),
        );
        r.git(
            &r.main,
            &["worktree", "remove", "--force", gone.to_str().unwrap()],
        );
        let before = r.snapshot(&default_path);
        for args in [
            vec!["gc", "--orphaned", "--json"],
            vec!["gc", "--merged", "--apply", "--json"],
        ] {
            let out = r.rabi(&r.main, &args);
            assert!(
                out.status.success(),
                "{} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert!(default_path.exists(), "{doc}");
            assert_eq!(r.snapshot(&default_path), before);
            if args.contains(&"--orphaned") {
                assert!(
                    doc["data"]["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["name"] == "gone"),
                    "{doc}"
                );
            }
        }
    }
}

#[test]
fn gc_orphaned_survives_unresolvable_default_mapping() {
    let r = Repo::new();
    let gone = r.create("gone");
    r.git(
        &r.main,
        &["worktree", "remove", "--force", gone.to_str().unwrap()],
    );
    r.git(
        &r.main,
        &["update-ref", "refs/remotes/unmapped/main", "HEAD"],
    );
    r.git(
        &r.main,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/unmapped/main",
        ],
    );
    let out = r.rabi(&r.main, &["gc", "--orphaned", "--apply", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["items"][0]["name"], "gone");
    assert_eq!(doc["data"]["items"][0]["action"], "removed");
}

#[test]
fn gc_unresolved_default_keeps_branch_worktrees_and_only_cleans_orphans() {
    for missing_base in [false, true] {
        let r = Repo::new();
        let tree = r.create("possibly-default");
        let gone = r.create("gone");
        r.git(
            &r.main,
            &["worktree", "remove", "--force", gone.to_str().unwrap()],
        );
        if missing_base {
            r.git(&r.main, &["branch", "-m", "develop"]);
            r.git(&r.main, &["remote", "remove", "origin"]);
        } else {
            r.git(
                &r.main,
                &["update-ref", "refs/remotes/unmapped/main", "HEAD"],
            );
            r.git(
                &r.main,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/unmapped/main",
                ],
            );
        }
        let store = r.main.join(".git/rabi/workspaces");
        let mut doc: Value = serde_json::from_slice(&records::read(&store).unwrap()).unwrap();
        for record in doc["worktrees"].as_array_mut().unwrap() {
            record["createdAt"] = json!("2000-01-01T00:00:00.000Z");
        }
        records::write(&store, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
        let before = r.snapshot(&tree);
        let out = r.rabi(
            &r.main,
            &[
                "gc",
                "--older-than",
                "1d",
                "--orphaned",
                "--apply",
                "--json",
            ],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .contains("cannot resolve the default branch; keeping all branch workspaces"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        let candidates = doc["data"]["items"].as_array().unwrap();
        assert!(
            candidates
                .iter()
                .all(|item| item["name"] != "possibly-default"),
            "age alone is not evidence: {doc}"
        );
        assert!(
            candidates
                .iter()
                .any(|item| item["name"] == "gone" && item["action"] == "removed"),
            "{doc}"
        );
        assert_eq!(r.snapshot(&tree), before);
    }
}

#[test]
fn sync_never_runs_fsmonitor_for_noop_update_or_conflict_abort() {
    for mode in [
        "noop",
        "rebase",
        "merge",
        "rebase-conflict",
        "merge-conflict",
    ] {
        let r = Repo::new();
        let tree = r.create("topic");
        let conflict = mode.ends_with("conflict");
        r.write_commit(&tree, if conflict { "file" } else { "own" }, "own\n");
        if mode != "noop" {
            if conflict {
                r.write_commit(&r.upstream, "file", "conflicting remote\n");
                r.git(&r.upstream, &["push", "origin", "main"]);
            } else {
                r.advance();
            }
        }
        let before = r.snapshot(&tree);
        let marker = r.install_fsmonitor();
        let mut args = vec!["topic"];
        if mode.starts_with("merge") {
            args.push("--merge");
        }
        let doc = r.sync(&r.main, &args, if conflict { 5 } else { 0 });
        assert_eq!(
            doc["data"]["items"][0]["status"],
            if conflict {
                "conflict"
            } else if mode == "noop" {
                "up-to-date"
            } else {
                "updated"
            }
        );
        assert!(!marker.exists(), "fsmonitor ran for {mode}");
        r.sync(&r.main, &[], 0);
        assert!(!marker.exists(), "fsmonitor ran for default-only {mode}");
        r.git(&r.main, &["config", "core.fsmonitor", "false"]);
        if conflict {
            assert_eq!(r.snapshot(&tree), before);
        }
    }
}

#[test]
fn rebase_keeps_own_commits_after_upstream_rewind_with_forkpoint_config() {
    for explicit in [false, true] {
        let r = Repo::new();
        let initial = r.git(&r.upstream, &["rev-parse", "HEAD"]);
        r.write_commit(&r.upstream, "old-upstream", "must survive rewind\n");
        r.git(&r.upstream, &["push", "origin", "main"]);
        r.git(&r.main, &["fetch", "origin"]);
        r.git(&r.main, &["merge", "--ff-only", "origin/main"]);
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "own\n");
        r.git(&r.upstream, &["reset", "--hard", &initial]);
        r.write_commit(&r.upstream, "new-upstream", "new upstream\n");
        r.git(&r.upstream, &["push", "--force", "origin", "main"]);
        r.git(&tree, &["config", "rebase.forkPoint", "true"]);
        let args = if explicit { vec!["--rebase"] } else { vec![] };
        let doc = r.sync(&tree, &args, 0);
        assert_eq!(doc["data"]["items"][0]["status"], "updated");
        assert_eq!(
            fs::read_to_string(tree.join("old-upstream")).unwrap(),
            "must survive rewind\n"
        );
        assert_eq!(fs::read_to_string(tree.join("own")).unwrap(), "own\n");
    }
}

#[test]
fn adopt_without_base_skips_sync() {
    check_adopted_sync_base(false);
}

#[test]
fn adopt_with_base_syncs_normally() {
    check_adopted_sync_base(true);
}

fn check_adopted_sync_base(explicit_base: bool) {
    for selector in ["topic", "--all"] {
        for no_default in [false, true] {
            let r = Repo::new();
            let tree = r.root.join("adopted");
            r.git(
                &r.main,
                &["worktree", "add", "-b", "topic", tree.to_str().unwrap()],
            );
            let mut args = vec!["adopt", tree.to_str().unwrap(), "--name", "topic", "--json"];
            if explicit_base {
                args.extend(["--base", "origin/main"]);
            }
            let adopted = r.rabi(&r.main, &args);
            assert!(adopted.status.success(), "{adopted:?}");
            let store = r.main.join(".git/rabi/workspaces");
            let record_before = records::read(&store).unwrap();
            r.advance();
            let before = r.snapshot(&tree);
            let mut args = vec![selector];
            if no_default {
                args.push("--main");
            }
            let doc = r.sync(&r.main, &args, 0);
            let items = doc["data"]["items"].as_array().unwrap();
            assert_eq!(items.len(), 1, "{doc}");
            let item = &items[0];
            if explicit_base {
                assert_eq!(item["status"], "updated", "{doc}");
                assert_eq!(item["base"], "refs/remotes/origin/main");
                assert_eq!(
                    r.git(&tree, &["rev-parse", "HEAD"]),
                    r.git(&r.upstream, &["rev-parse", "HEAD"])
                );
            } else {
                assert_eq!(item["status"], "base-unknown", "{doc}");
                assert!(item["base"].is_null());
                assert_eq!(r.snapshot(&tree), before);
                assert_eq!(records::read(&store).unwrap(), record_before);
                let record: Value = serde_json::from_slice(&record_before).unwrap();
                assert_eq!(record["worktrees"][0]["baseCleared"], true);
                assert!(record["worktrees"][0]["base"].is_null());
            }
        }
    }
}

#[test]
fn branch_with_unknown_default_stays_unknown_after_default_returns() {
    for selector in ["review", "--all"] {
        let r = Repo::new();
        r.git(&r.main, &["remote", "remove", "origin"]);
        r.git(&r.main, &["branch", "-m", "topic"]);
        r.git(&r.main, &["checkout", "--orphan", "unborn"]);
        let out = r.rabi(
            &r.main,
            &["create", "review", "--branch", "topic", "--json"],
        );
        assert!(out.status.success(), "{out:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(doc["data"]["workspace"]["base"].is_null());
        let tree = PathBuf::from(doc["data"]["workspace"]["path"].as_str().unwrap());
        // A newly available default must not silently replace the unknown intent.
        r.git(&r.main, &["checkout", "-b", "main", "topic"]);
        r.write_commit(&r.main, "new-default", "new default\n");
        let before = r.snapshot(&tree);
        let store = r.main.join(".git/rabi/workspaces");
        let record_before = records::read(&store).unwrap();
        let doc = r.sync(&r.main, &[selector], 0);
        assert_eq!(doc["data"]["items"][0]["status"], "base-unknown", "{doc}");
        assert_eq!(r.snapshot(&tree), before);
        assert_eq!(records::read(&store).unwrap(), record_before);
        let record: Value = serde_json::from_slice(&record_before).unwrap();
        assert_eq!(record["worktrees"][0]["baseCleared"], true);
        assert!(r
            .rabi(&r.main, &["set", "review", "--base", "main"])
            .status
            .success());
        let doc = r.sync(&r.main, &[selector], 0);
        assert_eq!(doc["data"]["items"][0]["status"], "updated", "{doc}");
    }
}

#[test]
fn branch_with_known_default_preserves_sync_fallback() {
    let r = Repo::new();
    r.git(&r.main, &["branch", "topic"]);
    let out = r.rabi(
        &r.main,
        &["create", "review", "--branch", "topic", "--json"],
    );
    assert!(out.status.success(), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(doc["data"]["workspace"]["base"].is_null());
    r.advance();
    let doc = r.sync(&r.main, &["review"], 0);
    assert_eq!(doc["data"]["items"][0]["status"], "updated", "{doc}");
}

#[test]
fn explicitly_cleared_base_skips_sync_until_reset() {
    for selector in ["--all", "topic"] {
        let r = Repo::new();
        let tree = r.create("topic");
        let out = r.rabi(&r.main, &["set", "topic", "--unset", "base", "--json"]);
        assert!(out.status.success());
        // An unrelated edit must preserve the explicit clear across process boundaries.
        assert!(r
            .rabi(&r.main, &["set", "topic", "--purpose", "paused"])
            .status
            .success());
        r.advance();
        let before = r.snapshot(&tree);
        let store = r.main.join(".git/rabi/workspaces");
        let record_before = records::read(&store).unwrap();
        let doc = r.sync(&r.main, &[selector], 0);
        let item = &doc["data"]["items"][0];
        assert_eq!(item["status"], "base-unknown");
        assert!(item["base"].is_null());
        assert!(item["detail"]
            .as_str()
            .unwrap()
            .contains("explicitly cleared"));
        assert!(item["detail"]
            .as_str()
            .unwrap()
            .contains("rabi set topic --base REF"));
        assert_eq!(r.snapshot(&tree), before);
        assert_eq!(records::read(&store).unwrap(), record_before);
        let out = r.rabi(&r.main, &["set", "topic", "--base", "origin/main"]);
        assert!(out.status.success());
        let doc = r.sync(&r.main, &[selector], 0);
        assert_eq!(doc["data"]["items"][0]["status"], "updated");
        assert_eq!(
            r.git(&tree, &["rev-parse", "HEAD"]),
            r.git(&r.upstream, &["rev-parse", "HEAD"])
        );
    }
}

#[test]
fn cleared_base_on_default_checkout_is_skipped() {
    for selector in ["--all", "topic"] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.git(&r.main, &["checkout", "-b", "primary"]);
        r.git(&tree, &["checkout", "main"]);
        assert!(r
            .rabi(&r.main, &["set", "topic", "--unset", "base"])
            .status
            .success());
        r.advance();
        let before = r.snapshot(&tree);
        let doc = r.sync(&r.main, &[selector], 0);
        assert_eq!(doc["data"]["items"][0]["status"], "base-unknown");
        assert_eq!(r.snapshot(&tree), before);
        let doc = r.sync(&r.main, &[selector], 0);
        assert_eq!(doc["data"]["items"][0]["status"], "base-unknown");
        assert_eq!(r.snapshot(&tree), before);
    }
}

fn assert_cleared_default_skips_unavailable_remote(selectors: &[&str]) {
    for no_default in [false, true] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.git(&r.main, &["checkout", "-b", "primary"]);
        r.git(&tree, &["checkout", "main"]);
        assert!(r.rabi(&tree, &["set", "--unset", "base"]).status.success());
        r.advance();
        r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
        let before = r.snapshot(&tree);
        let store = r.main.join(".git/rabi/workspaces");
        let records_before = records::read(&store).unwrap();
        let refs_before = r.git(&r.main, &["for-each-ref"]);
        let trace = r.root.join("trace");
        let mut args = vec!["sync", "--json"];
        args.extend_from_slice(selectors);
        if no_default {
            args.push("--main");
            if selectors.is_empty() {
                args.push(".");
            }
        }
        let out = r
            .command(env!("CARGO_BIN_EXE_rabi"), &tree, &args)
            .env("GIT_TRACE", &trace)
            .output()
            .unwrap();
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["schema"], 2);
        // Design §3.4: --main skips non-default primary, while linked main
        // preserves the explicitly cleared base without fetching.
        if no_default {
            assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
        } else {
            assert!(doc["data"]["main"].is_null());
        }
        let items = doc["data"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "base-unknown");
        assert!(items[0]["base"].is_null());
        assert_eq!(out.status.code(), Some(0), "{doc}");
        assert!(!fs::read_to_string(&trace)
            .unwrap()
            .contains("fetch --prune"));
        assert_eq!(r.snapshot(&tree), before);
        assert_eq!(records::read(&store).unwrap(), records_before);
        assert_eq!(r.git(&r.main, &["for-each-ref"]), refs_before);
    }
}

#[test]
fn cleared_default_named_sync_skips_unavailable_remote() {
    assert_cleared_default_skips_unavailable_remote(&["topic"]);
}

#[test]
fn cleared_default_all_sync_skips_unavailable_remote() {
    assert_cleared_default_skips_unavailable_remote(&["--all"]);
}

#[test]
fn cleared_default_implicit_sync_skips_unavailable_remote() {
    assert_cleared_default_skips_unavailable_remote(&[]);
}

#[test]
fn cleared_primary_default_skips_unavailable_remote() {
    for selectors in [vec!["."], vec!["--all", "--main"], vec![]] {
        let r = Repo::new();
        assert!(r
            .rabi(&r.main, &["set", "--unset", "base"])
            .status
            .success());
        r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
        let doc = r.sync(&r.main, &selectors, 0);
        assert_eq!(doc["data"]["main"]["status"], "base-unknown");
    }
}

#[test]
fn cleared_bases_do_not_hide_other_targets_fetch_failures() {
    for clear_default in [false, true] {
        for selectors in [vec!["topic"], vec!["--all"], vec!["."]] {
            let r = Repo::new();
            let tree = r.create("topic");
            let cleared = if clear_default { &r.main } else { &tree };
            assert!(r
                .rabi(cleared, &["set", "--unset", "base"])
                .status
                .success());
            r.git(&r.main, &["remote", "set-url", "origin", "/does/not/exist"]);
            let before = r.snapshot(&tree);
            let doc = r.sync(&tree, &[selectors.as_slice(), &["--main"]].concat(), 1);
            assert_eq!(doc["schema"], 2);
            let (unknown, failed) = if clear_default {
                (&doc["data"]["main"], &doc["data"]["items"][0])
            } else {
                (&doc["data"]["items"][0], &doc["data"]["main"])
            };
            assert_eq!(unknown["status"], "base-unknown", "{doc}");
            assert!(unknown["base"].is_null());
            assert_eq!(failed["status"], "failed", "{doc}");
            assert!(failed["detail"]
                .as_str()
                .unwrap()
                .contains("Fetch origin failed"));
            assert_eq!(r.snapshot(&tree), before);
        }
    }
}

#[test]
fn missing_base_falls_back_but_explicitly_cleared_base_stays_unknown() {
    let r = Repo::new();
    let fallback = r.root.join("fallback");
    let cleared = r.root.join("cleared");
    for (name, path) in [("fallback", &fallback), ("cleared", &cleared)] {
        r.git(
            &r.main,
            &["worktree", "add", "-b", name, path.to_str().unwrap()],
        );
    }
    for path in [&fallback, &cleared] {
        rabi::metadata::write(&r.main, path, &Default::default()).unwrap();
    }
    assert!(r
        .rabi(&r.main, &["set", "cleared", "--unset", "base"])
        .status
        .success());
    r.advance();
    let before = r.snapshot(&cleared);
    let doc = r.sync(&r.main, &["--all"], 0);
    let items = doc["data"]["items"].as_array().unwrap();
    let old = items
        .iter()
        .find(|item| item["name"] == "fallback")
        .unwrap();
    assert_eq!(old["base"], "refs/remotes/origin/main");
    assert_eq!(old["status"], "updated");
    let unknown = items.iter().find(|item| item["name"] == "cleared").unwrap();
    assert_eq!(unknown["status"], "base-unknown");
    assert_eq!(r.snapshot(&cleared), before);
}

// P8 design §3.4 and §4: implicit workspace sync never advances primary.
#[test]
fn p8_current_only_main_and_all_scopes() {
    let r = Repo::new();
    let tree = r.create("topic");
    let other = r.create("other");
    let primary = r.snapshot(&r.main);
    let other_before = r.snapshot(&other);
    r.advance();
    let doc = r.sync(&tree, &[], 0);
    assert!(doc["data"]["main"].is_null(), "{doc}");
    assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(doc["data"]["items"][0]["workspace"]["name"], "topic");
    assert_eq!(r.snapshot(&r.main), primary);
    assert_eq!(r.snapshot(&other), other_before);
    let doc = r.sync(&tree, &[".", "--main"], 0);
    assert_eq!(doc["data"]["main"]["status"], "updated");
    let doc = r.sync(&tree, &["--all"], 0);
    assert!(doc["data"]["main"].is_null());
    assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 2);
    r.sync(&tree, &["topic", "--all"], 2);
    r.git(&r.main, &["checkout", "-b", "primary-topic"]);
    let refs = r.git(&r.main, &["for-each-ref"]);
    let doc = r.sync(&tree, &["--main"], 0);
    assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
    assert_eq!(r.git(&r.main, &["for-each-ref"]), refs);
}

#[test]
fn p8_published_rewrite_permission_and_provenance() {
    for permission in ["refuse", "flag", "config"] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "own\n");
        r.git(&tree, &["push", "-u", "origin", "topic"]);
        let before = r.snapshot(&tree);
        let upstream = r.git(&tree, &["rev-parse", "@{upstream}"]);
        r.advance();
        let mut args = vec!["--rebase"];
        if permission == "flag" {
            args.push("--rewrite-published");
        } else if permission == "config" {
            let out = r.rabi(
                &tree,
                &["config", "set", "sync.rewrite-published", "true", "--json"],
            );
            assert!(out.status.success(), "{out:?}");
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(doc["data"]["value"], true);
        }
        let doc = r.sync(&tree, &args, if permission == "refuse" { 4 } else { 0 });
        let item = &doc["data"]["items"][0];
        assert_eq!(item["strategy"], "rebase");
        assert_eq!(item["strategySource"], "flag");
        assert_eq!(item["rewriteAllowed"], permission != "refuse");
        assert_eq!(
            item["rewriteSource"],
            if permission == "refuse" {
                Value::Null
            } else {
                json!(permission)
            }
        );
        assert_eq!(item["rewrotePublished"], permission != "refuse");
        if permission == "refuse" {
            assert_eq!(doc["error"]["code"], "published");
            assert_eq!(item["status"], "refused");
            assert!(item["detail"]
                .as_str()
                .unwrap()
                .contains("--rewrite-published"));
            assert_eq!(r.snapshot(&tree), before);
        } else {
            let warning = &doc["warnings"][0];
            assert_eq!(warning["code"], "published_rewritten");
            assert_eq!(warning["workspace"], "topic");
            assert!(warning["message"]
                .as_str()
                .unwrap()
                .contains("updating the existing remote branch may require a force push"));
        }
        assert_eq!(r.git(&tree, &["rev-parse", "@{upstream}"]), upstream);
        assert_eq!(
            r.git(&tree, &["config", "--get", "branch.topic.remote"]),
            "origin"
        );
    }
}

#[test]
fn p8_strategy_sources_and_permission_does_not_choose_rebase() {
    for (configured, flag, strategy, source) in [
        (None, None, "rebase", "default"),
        (Some("merge"), None, "merge", "config"),
        (Some("merge"), Some("--rebase"), "rebase", "flag"),
        (Some("merge"), Some("--auto"), "rebase", "flag"),
        (Some("rebase"), Some("--merge"), "merge", "flag"),
    ] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "own\n");
        if let Some(value) = configured {
            assert!(r
                .rabi(&tree, &["config", "set", "sync.strategy", value])
                .status
                .success());
        }
        r.advance();
        let args: Vec<_> = flag.into_iter().collect();
        let doc = r.sync(&tree, &args, 0);
        assert_eq!(doc["data"]["items"][0]["strategy"], strategy);
        assert_eq!(doc["data"]["items"][0]["strategySource"], source);
    }
    let r = Repo::new();
    let tree = r.create("published");
    r.write_commit(&tree, "own", "own\n");
    r.git(&tree, &["push", "origin", "published"]);
    r.advance();
    let doc = r.sync(&tree, &["--rewrite-published"], 0);
    assert_eq!(doc["data"]["items"][0]["strategy"], "merge");
    assert_eq!(doc["data"]["items"][0]["strategySource"], "default");
    assert_eq!(doc["data"]["items"][0]["rewrotePublished"], false);
    assert_eq!(doc["warnings"], json!([]));
}

fn assert_fetch_refused(r: &Repo, path: &Path, dry_run: bool, scope: &[&str], fragments: &[&str]) {
    let refs = r.git(&r.main, &["for-each-ref"]);
    let reflogs = file_contents(&r.main.join(".git/logs"));
    let fetch_head = fs::read(r.main.join(".git/FETCH_HEAD")).unwrap();
    let primary = r.snapshot(&r.main);
    let checkout = r.snapshot(path);
    let record_dir = r.main.join(".git/rabi/workspaces");
    let records = record_dir.exists().then(|| file_contents(&record_dir));
    let trace = r.root.join("refusal-trace");
    let mut args = vec!["sync", "--json"];
    args.extend_from_slice(scope);
    if dry_run {
        args.push("--dry-run");
    }
    let out = r
        .command(env!("CARGO_BIN_EXE_rabi"), path, &args)
        .env("GIT_TRACE", &trace)
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(4), "{doc}");
    assert_eq!(doc["error"]["code"], "refused", "{doc}");
    for fragment in fragments {
        assert!(
            doc["error"]["message"].as_str().unwrap().contains(fragment),
            "{fragment}: {doc}"
        );
    }
    assert!(!fs::read_to_string(trace).unwrap().contains(" fetch "));
    assert_eq!(r.git(&r.main, &["for-each-ref"]), refs);
    assert_eq!(file_contents(&r.main.join(".git/logs")), reflogs);
    assert_eq!(
        fs::read(r.main.join(".git/FETCH_HEAD")).unwrap(),
        fetch_head
    );
    assert_eq!(r.snapshot(&r.main), primary);
    assert_eq!(r.snapshot(path), checkout);
    assert_eq!(
        record_dir.exists().then(|| file_contents(&record_dir)),
        records
    );
}

#[test]
fn review_p1_fetch_cannot_rewrite_local_branches_or_their_reflogs() {
    for dry_run in [true, false] {
        for destination in ["refs/heads/release", "refs/heads/rel*", "refs/*"] {
            let r = Repo::new();
            r.git(&r.main, &["checkout", "-b", "release"]);
            r.write_commit(&r.main, "local-release", "unpublished release\n");
            r.git(&r.main, &["checkout", "main"]);
            r.git(&r.upstream, &["checkout", "-b", "release"]);
            r.write_commit(&r.upstream, "remote-release", "remote release\n");
            r.git(&r.upstream, &["push", "origin", "release"]);
            let source = match destination {
                "refs/heads/rel*" => "refs/heads/rel*",
                "refs/*" => "refs/*",
                _ => "refs/heads/release",
            };
            r.git(
                &r.main,
                &[
                    "config",
                    "--add",
                    "remote.origin.fetch",
                    &format!("+{source}:{destination}"),
                ],
            );
            r.advance();
            assert_fetch_refused(
                &r,
                &r.main,
                dry_run,
                &[],
                &["origin", &format!("+{source}:{destination}"), "Configure"],
            );
        }
    }
}

#[test]
fn conservative_fetch_refuses_destinations_outside_tracking_namespaces() {
    for dry_run in [true, false] {
        for destination in [
            "refs/cache/*",
            "refs/remotes-other/*",
            "refs/tags-other/*",
            "refs/tags/cache/*",
            "refs/tags/cache/main",
            "release",
        ] {
            let r = Repo::new();
            let source = if destination.contains('*') {
                "refs/heads/*"
            } else {
                "refs/heads/main"
            };
            let refspec = format!("+{source}:{destination}");
            r.git(
                &r.main,
                &["config", "--add", "remote.origin.fetch", &refspec],
            );
            r.advance();
            assert_fetch_refused(
                &r,
                &r.main,
                dry_run,
                &[],
                &["origin", &refspec, "Configure"],
            );
        }
    }
}

#[test]
fn conservative_fetch_supports_custom_tracking_namespaces_in_plans_and_execution() {
    let r = Repo::new();
    r.git(
        &r.main,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/custom/*",
        ],
    );
    r.advance();
    let heads = r.git(&r.main, &["for-each-ref", "refs/heads"]);
    let reflogs = file_contents(&r.main.join(".git/logs/refs/heads"));
    let primary = r.snapshot(&r.main);
    let plan = r.sync(&r.main, &["--dry-run"], 0);
    assert_eq!(plan["data"]["main"]["status"], "updated");
    assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), heads);
    assert_eq!(file_contents(&r.main.join(".git/logs/refs/heads")), reflogs);
    assert_eq!(r.snapshot(&r.main), primary);
    assert_eq!(
        r.git(&r.main, &["rev-parse", "refs/remotes/custom/main"]),
        r.git(&r.upstream, &["rev-parse", "HEAD"])
    );
    let actual = r.sync(&r.main, &[], 0);
    assert_eq!(actual["data"]["main"]["status"], "updated");
    assert_eq!(
        actual["data"]["main"]["after"],
        plan["data"]["main"]["after"]
    );
}

#[test]
fn review_p1_fetch_cannot_follow_tracking_symrefs_into_local_branches() {
    for dry_run in [true, false] {
        let r = Repo::new();
        r.git(&r.main, &["branch", "release"]);
        r.git(
            &r.main,
            &[
                "symbolic-ref",
                "refs/remotes/origin/release",
                "refs/heads/release",
            ],
        );
        let tree = r.create("topic");
        fs::write(tree.join("file"), "dirty workspace\n").unwrap();
        r.advance();
        r.git(&r.upstream, &["push", "origin", "HEAD:release"]);
        assert_fetch_refused(
            &r,
            &tree,
            dry_run,
            &[],
            &[
                "refs/remotes/origin/release",
                "refs/heads/release",
                "Repair",
            ],
        );
    }
}

#[test]
fn conservative_fetch_refuses_unsafe_destinations_even_with_negative_exclusions() {
    for dry_run in [true, false] {
        let r = Repo::new();
        let tree = r.create("topic");
        fs::write(tree.join("file"), "dirty workspace\n").unwrap();
        r.git(&r.main, &["branch", "release"]);
        r.git(
            &r.main,
            &["update-ref", "refs/remotes/origin/release", "HEAD"],
        );
        r.git(
            &r.main,
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "+refs/heads/release:refs/heads/release",
            ],
        );
        r.git(
            &r.main,
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "^refs/heads/release",
            ],
        );
        r.advance();
        r.git(&r.upstream, &["push", "origin", "HEAD:release"]);
        assert_fetch_refused(
            &r,
            &tree,
            dry_run,
            &[],
            &["origin", "+refs/heads/release:refs/heads/release"],
        );
    }
}

#[test]
fn conservative_fetch_checks_all_involved_remotes_before_any_fetch() {
    for dry_run in [true, false] {
        let r = Repo::new();
        r.git(
            &r.main,
            &[
                "remote",
                "add",
                "z-other",
                r.root.join("remote.git").to_str().unwrap(),
            ],
        );
        r.git(&r.main, &["fetch", "z-other"]);
        let a = r.create("a");
        let b = r.create("b");
        assert!(r
            .rabi(&b, &["set", "--base", "refs/remotes/z-other/main"])
            .status
            .success());
        r.git(
            &r.main,
            &[
                "config",
                "--add",
                "remote.z-other.fetch",
                "+refs/heads/main:refs/heads/unselected",
            ],
        );
        r.advance();
        assert_fetch_refused(
            &r,
            &r.main,
            dry_run,
            &["a", "b", "--main"],
            &["z-other", "+refs/heads/main:refs/heads/unselected"],
        );
        assert!(!a.join("remote-file").exists());
        assert!(!b.join("remote-file").exists());
    }
}

#[test]
fn conservative_fetch_allows_missing_and_empty_destinations() {
    for dry_run in [true, false] {
        for refspec in ["refs/heads/main", "refs/heads/main:"] {
            let r = Repo::new();
            let tree = r.create("topic");
            fs::write(tree.join("file"), "dirty\n").unwrap();
            r.git(
                &r.main,
                &["config", "--add", "remote.origin.fetch", refspec],
            );
            let heads = r.git(&r.main, &["for-each-ref", "refs/heads"]);
            r.advance();
            let tip = r.git(&r.upstream, &["rev-parse", "HEAD"]);
            let doc = r.sync(&tree, if dry_run { &["--dry-run"] } else { &[] }, 0);
            assert_eq!(doc["data"]["items"][0]["status"], "dirty", "{doc}");
            assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), heads);
            assert_eq!(r.git(&r.main, &["rev-parse", "origin/main"]), tip);
            assert!(fs::read_to_string(r.main.join(".git/FETCH_HEAD"))
                .unwrap()
                .contains(&tip));
        }
    }
}

#[test]
fn conservative_fetch_refuses_visible_symrefs_to_any_outside_namespace() {
    for dry_run in [true, false] {
        for target in ["refs/tags/existing", "refs/cache/existing"] {
            let r = Repo::new();
            r.git(&r.main, &["update-ref", target, "HEAD"]);
            // Refuse even an alias unrelated to the fetched remote/refspec.
            r.git(
                &r.main,
                &["symbolic-ref", "refs/remotes/unrelated/alias", target],
            );
            r.advance();
            assert_fetch_refused(
                &r,
                &r.main,
                dry_run,
                &[],
                &["refs/remotes/unrelated/alias", target, "Repair"],
            );
        }
    }
}

#[test]
fn dry_run_propagates_earlier_workspace_tips_through_local_bases() {
    let r = Repo::new();
    let a = r.create("a");
    let b = r.create("b");
    let c = r.create("c");
    assert!(r.rabi(&b, &["set", "--base", "a"]).status.success());
    assert!(r.rabi(&c, &["set", "--base", "b"]).status.success());
    let before = [r.snapshot(&a), r.snapshot(&b), r.snapshot(&c)];
    let refs = r.git(&r.main, &["for-each-ref", "refs/heads"]);
    let records = file_contents(&r.main.join(".git/rabi/workspaces"));
    r.advance();
    let plan = r.sync(&r.main, &["a", "b", "c", "--dry-run"], 0);
    assert_eq!([r.snapshot(&a), r.snapshot(&b), r.snapshot(&c)], before);
    assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), refs);
    assert_eq!(file_contents(&r.main.join(".git/rabi/workspaces")), records);
    let actual = r.sync(&r.main, &["a", "b", "c"], 0);
    for i in 0..3 {
        let planned = &plan["data"]["items"][i];
        let executed = &actual["data"]["items"][i];
        assert_eq!(planned["status"], "updated", "{plan}");
        for field in ["status", "strategy", "base", "before", "after"] {
            assert_eq!(
                planned[field], executed[field],
                "{field}: plan={plan}, actual={actual}"
            );
        }
    }
}

#[test]
fn dry_run_propagates_a_branch_named_like_a_full_ref() {
    check_ref_prefixed_branch_plan("refs/heads/refs/heads/a", "updated");
}

#[test]
fn dry_run_does_not_alias_a_ref_prefixed_branch_to_another_branch() {
    check_ref_prefixed_branch_plan("refs/heads/a", "up-to-date");
}

fn check_ref_prefixed_branch_plan(dependent_base: &str, expected_status: &str) {
    for unknown_tip in [false, true] {
        let r = Repo::new();
        let a = r.create("a");
        let dependent = r.create("dependent");
        // Record a's full ref before Git's DWIM rules can also match
        // refs/heads/refs/heads/a for the input refs/heads/a.
        assert!(r.rabi(&dependent, &["set", "--base", "a"]).status.success());
        let prefixed = r.create("refs/heads/a");
        if unknown_tip {
            r.write_commit(&prefixed, "own", "unpublished\n");
        }
        if dependent_base != "refs/heads/a" {
            r.git(&dependent, &["reset", "--hard", dependent_base]);
            assert!(r
                .rabi(&dependent, &["set", "--base", dependent_base])
                .status
                .success());
            if unknown_tip {
                r.write_commit(&dependent, "dependent-own", "unpublished dependency\n");
            }
        }
        let before = [
            r.snapshot(&a),
            r.snapshot(&prefixed),
            r.snapshot(&dependent),
        ];
        let refs = r.git(&r.main, &["for-each-ref", "refs/heads"]);
        let records = file_contents(&r.main.join(".git/rabi/workspaces"));
        r.advance();
        let plan = r.sync(&r.main, &["refs/heads/a", "dependent", "--dry-run"], 0);
        assert_eq!(
            [
                r.snapshot(&a),
                r.snapshot(&prefixed),
                r.snapshot(&dependent)
            ],
            before
        );
        assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), refs);
        assert_eq!(file_contents(&r.main.join(".git/rabi/workspaces")), records);
        let actual = r.sync(&r.main, &["refs/heads/a", "dependent"], 0);
        assert_eq!(actual["data"]["items"][1]["status"], expected_status);
        assert_eq!(r.snapshot(&a), before[0]);
        assert_eq!(plan["data"]["items"][0]["status"], "updated", "{plan}");
        let planned = &plan["data"]["items"][1];
        assert_eq!(planned["base"], dependent_base);
        assert_eq!(planned["status"], expected_status, "{plan}");
        if unknown_tip && expected_status == "updated" {
            assert!(planned["after"].is_null(), "{plan}");
            assert!(planned["strategy"].is_null(), "{plan}");
            assert!(planned["detail"]
                .as_str()
                .unwrap()
                .contains(&format!("depends on planned update of {dependent_base}")));
        } else {
            for field in ["status", "strategy", "base", "before", "after"] {
                assert_eq!(
                    planned[field], actual["data"]["items"][1][field],
                    "{field}: plan={plan}, actual={actual}"
                );
            }
        }
    }
}

#[test]
fn git_prune_negative_exclusions_tags_and_remote_head_remain_scoped() {
    for dry_run in [true, false] {
        let r = Repo::new();
        let tree = r.create("topic");
        fs::write(tree.join("file"), "dirty workspace\n").unwrap();
        r.git(
            &r.upstream,
            &["push", "origin", "HEAD:gone", "HEAD:excluded"],
        );
        r.git(
            &r.main,
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/cache/*",
            ],
        );
        r.git(
            &r.main,
            &["config", "remote.origin.followRemoteHEAD", "always"],
        );
        r.git(&r.main, &["fetch", "origin"]);
        r.git(&r.main, &["update-ref", "refs/other/untouched", "HEAD"]);
        r.git(&r.main, &["tag", "local-only"]);
        let untouched = r.git(&r.main, &["rev-parse", "refs/other/untouched"]);
        let excluded = r.git(&r.main, &["rev-parse", "refs/remotes/origin/excluded"]);
        let heads = r.git(&r.main, &["for-each-ref", "refs/heads"]);
        let reflogs = file_contents(&r.main.join(".git/logs/refs/heads"));
        r.git(
            &r.main,
            &[
                "config",
                "--add",
                "remote.origin.fetch",
                "^refs/heads/excl*",
            ],
        );
        r.git(&r.upstream, &["push", "origin", ":gone", ":excluded"]);
        r.advance();
        r.git(&r.upstream, &["tag", "fetched-tag"]);
        r.git(&r.upstream, &["push", "origin", "refs/tags/fetched-tag"]);
        let doc = r.sync(&tree, if dry_run { &["--dry-run"] } else { &[] }, 0);
        assert_eq!(doc["data"]["items"][0]["status"], "dirty", "{doc}");
        assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), heads);
        assert_eq!(file_contents(&r.main.join(".git/logs/refs/heads")), reflogs);
        assert_eq!(
            r.git(
                &r.main,
                &[
                    "for-each-ref",
                    "refs/remotes/origin/gone",
                    "refs/remotes/cache/gone"
                ]
            ),
            ""
        );
        assert_eq!(
            r.git(&r.main, &["rev-parse", "refs/remotes/origin/excluded"]),
            excluded
        );
        assert_eq!(
            r.git(&r.main, &["rev-parse", "refs/remotes/cache/excluded"]),
            excluded
        );
        assert_eq!(
            r.git(&r.main, &["rev-parse", "refs/other/untouched"]),
            untouched
        );
        assert_eq!(r.git(&r.main, &["rev-parse", "local-only"]), untouched);
        assert_eq!(
            r.git(&r.main, &["rev-parse", "refs/tags/fetched-tag"]),
            r.git(&r.upstream, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            r.git(&r.main, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
            "refs/remotes/origin/main"
        );
    }
}

#[test]
fn effective_conditional_and_environment_fetch_config_is_retained() {
    let r = Repo::new();
    let tree = r.create("topic");
    // The remote mapping exists only in a condition matching the original Git
    // directory. Fetch and preflight must both use this effective configuration.
    r.git(&r.main, &["config", "--unset-all", "remote.origin.fetch"]);
    let included = r.root.join("conditional.config");
    r.git(
        &r.main,
        &[
            "config",
            "--file",
            included.to_str().unwrap(),
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/cache/*",
        ],
    );
    r.git(
        &r.main,
        &[
            "config",
            &format!("includeIf.gitdir:{}.path", r.main.join(".git").display()),
            included.to_str().unwrap(),
        ],
    );
    let original = fs::read(r.main.join(".git/config")).unwrap();
    r.advance();
    let out = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &tree,
            &["sync", "--dry-run", "--json"],
        )
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.origin.fetch")
        .env("GIT_CONFIG_VALUE_0", "+refs/heads/*:refs/remotes/origin/*")
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{doc}");
    assert_eq!(doc["data"]["items"][0]["status"], "updated");
    assert_eq!(
        r.git(&r.main, &["rev-parse", "refs/remotes/cache/main"]),
        r.git(&r.upstream, &["rev-parse", "HEAD"])
    );
    // Reading effective configuration must not rewrite the original config.
    assert_eq!(fs::read(r.main.join(".git/config")).unwrap(), original);
    assert_eq!(
        r.git(&r.main, &["config", "--get-all", "remote.origin.fetch"]),
        "+refs/heads/*:refs/remotes/cache/*"
    );
}

#[test]
fn review_p2_implicit_current_rejects_a_different_repository() {
    let a = Repo::new();
    let tree = a.create("topic");
    let b = Repo::new();
    b.advance();
    let primary = b.snapshot(&b.main);
    let mut errors = Vec::new();
    for selectors in [vec![], vec!["."]] {
        let mut args = vec!["-R", b.main.to_str().unwrap(), "sync", "--json"];
        args.extend(selectors);
        let out = a.rabi(&tree, &args);
        assert_eq!(b.snapshot(&b.main), primary);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(2), "{doc}");
        assert_eq!(doc["error"]["code"], "usage");
        errors.push(doc["error"].clone());
    }
    assert_eq!(errors[0], errors[1]);
}

#[test]
fn review_p2_dry_run_uses_the_planned_primary_tip_as_local_base() {
    for own_work in ["none", "unpublished", "published"] {
        for dirty_primary in [false, true] {
            let r = Repo::new();
            let out = r.rabi(&r.main, &["create", "topic", "--base", "main", "--json"]);
            assert!(out.status.success(), "{out:?}");
            let created: Value = serde_json::from_slice(&out.stdout).unwrap();
            let tree = PathBuf::from(created["data"]["workspace"]["path"].as_str().unwrap());
            assert_eq!(created["data"]["workspace"]["base"], "refs/heads/main");
            if own_work != "none" {
                r.write_commit(&tree, "own", "workspace work\n");
            }
            if own_work == "published" {
                r.git(&tree, &["push", "-u", "origin", "topic"]);
            }
            if dirty_primary {
                fs::write(r.main.join("file"), "dirty primary\n").unwrap();
            }
            let primary = r.snapshot(&r.main);
            let workspace = r.snapshot(&tree);
            let reflogs = file_contents(&r.main.join(".git/logs/refs/heads"));
            let records = file_contents(&r.main.join(".git/rabi/workspaces"));
            r.advance();
            let plan = r.sync(&tree, &[".", "--main", "--dry-run"], 0);
            assert_eq!(r.snapshot(&r.main), primary);
            assert_eq!(r.snapshot(&tree), workspace);
            assert_eq!(file_contents(&r.main.join(".git/logs/refs/heads")), reflogs);
            assert_eq!(file_contents(&r.main.join(".git/rabi/workspaces")), records);
            let actual = r.sync(&tree, &[".", "--main"], 0);
            for (planned, executed) in [
                (&plan["data"]["main"], &actual["data"]["main"]),
                (&plan["data"]["items"][0], &actual["data"]["items"][0]),
            ] {
                for field in [
                    "status",
                    "strategy",
                    "strategySource",
                    "base",
                    "before",
                    "code",
                ] {
                    assert_eq!(
                        planned[field], executed[field],
                        "{field}: plan={plan}, actual={actual}"
                    );
                }
                if !planned["after"].is_null() {
                    assert_eq!(planned["after"], executed["after"]);
                }
            }
            assert_eq!(
                plan["data"]["items"][0]["status"],
                if dirty_primary {
                    "up-to-date"
                } else {
                    "updated"
                }
            );
            if own_work != "none" && !dirty_primary {
                assert!(plan["data"]["items"][0]["after"].is_null());
            }
        }
    }
}

#[test]
fn p8_dry_run_fetches_without_checkout_or_metadata_changes() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "own\n");
    let snapshots = [r.snapshot(&r.main), r.snapshot(&tree)];
    let records_path = r.main.join(".git/rabi/workspaces");
    let metadata = file_contents(&records_path);
    let heads = r.git(&tree, &["for-each-ref", "refs/heads"]);
    let index_time = fs::metadata(r.main.join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    r.advance();
    let tip = r.git(&r.upstream, &["rev-parse", "HEAD"]);
    let doc = r.sync(&tree, &[".", "--main", "--dry-run"], 0);
    assert_eq!(
        fs::metadata(r.main.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap(),
        index_time
    );
    assert_eq!(doc["data"]["plan"], true);
    assert_eq!(doc["data"]["main"]["status"], "updated");
    assert_eq!(doc["data"]["items"][0]["status"], "updated");
    assert_eq!(r.git(&tree, &["rev-parse", "origin/main"]), tip);
    assert_eq!(r.git(&tree, &["for-each-ref", "refs/heads"]), heads);
    assert_eq!(r.snapshot(&r.main), snapshots[0]);
    assert_eq!(r.snapshot(&tree), snapshots[1]);
    assert_eq!(file_contents(&records_path), metadata);
}

#[test]
fn sync_config_validation() {
    let r = Repo::new();
    for (key, value) in [
        ("sync.strategy", "invalid"),
        ("sync.rewrite-published", "yes"),
    ] {
        let out = r.rabi(&r.main, &["config", "set", key, value, "--json"]);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
    }
    for (key, value, default) in [
        ("sync.strategy", "merge", json!("auto")),
        ("sync.rewrite-published", "true", json!(false)),
    ] {
        assert!(r
            .rabi(&r.main, &["config", "set", key, value])
            .status
            .success());
        let out = r.rabi(&r.main, &["config", "list", "--sources", "--json"]);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["data"]["config"][key]["source"], "config");
        assert!(r.rabi(&r.main, &["config", "unset", key]).status.success());
        let out = r.rabi(&r.main, &["config", "get", key, "--sources", "--json"]);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["data"]["config"][key]["value"], default);
        assert_eq!(doc["data"]["config"][key]["source"], "default");
    }
    fs::write(
        r.main.join(".rabi.toml"),
        "[sync]\nrewrite-published = true\n",
    )
    .unwrap();
    let out = r.rabi(&r.main, &["sync", "--json"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("sync"));
}

#[test]
fn p8_config_policy_permission_sources_remain_independent() {
    for (
        configured_strategy,
        permission,
        flag,
        expected_strategy,
        expected_strategy_source,
        expected_permission_source,
        exit,
    ) in [
        ("rebase", false, None, "rebase", "config", "config", 4),
        ("rebase", true, None, "rebase", "config", "config", 0),
        (
            "rebase",
            false,
            Some("--rewrite-published"),
            "rebase",
            "config",
            "flag",
            0,
        ),
        ("auto", true, None, "merge", "config", "config", 0),
        ("rebase", true, Some("--auto"), "merge", "flag", "config", 0),
    ] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "own", "published work\n");
        r.git(&tree, &["push", "-u", "origin", "topic"]);
        assert!(r
            .rabi(
                &tree,
                &["config", "set", "sync.strategy", configured_strategy]
            )
            .status
            .success());
        assert!(r
            .rabi(
                &tree,
                &[
                    "config",
                    "set",
                    "sync.rewrite-published",
                    if permission { "true" } else { "false" }
                ]
            )
            .status
            .success());
        r.advance();
        let flags: Vec<_> = flag.into_iter().collect();
        let doc = r.sync(&tree, &flags, exit);
        let item = &doc["data"]["items"][0];
        assert_eq!(item["strategy"], expected_strategy);
        assert_eq!(item["strategySource"], expected_strategy_source);
        assert_eq!(
            item["rewriteAllowed"],
            permission || flag == Some("--rewrite-published")
        );
        assert_eq!(item["rewriteSource"], expected_permission_source);
        assert_eq!(
            item["rewrotePublished"],
            exit == 0 && expected_strategy == "rebase"
        );
    }
}

#[test]
fn p8_invalid_policy_in_user_file_is_usage_and_repo_policy_is_rejected() {
    let r = Repo::new();
    let path = r.root.join("config/rabi/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for content in [
        "[sync]\nstrategy = 'unknown'\n",
        "[sync]\nstrategy = 123\n",
        "[sync]\nrewrite-published = 'true'\n",
    ] {
        fs::write(&path, content).unwrap();
        for args in [vec!["config", "list", "--json"], vec!["sync", "--json"]] {
            let out = r.rabi(&r.main, &args);
            assert_eq!(out.status.code(), Some(2), "{content}: {out:?}");
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(doc["error"]["code"], "usage");
        }
    }
    fs::remove_file(path).unwrap();
    for content in [
        "[sync]\nstrategy = 'merge'\n",
        "[sync]\nrewrite-published = true\n",
    ] {
        fs::write(r.main.join(".rabi.toml"), content).unwrap();
        let out = r.rabi(&r.main, &["sync", "--json"]);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("unknown field"));
    }
}

#[test]
fn p8_dry_run_deduplicates_fetch_and_never_claims_a_rewrite() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "published work\n");
    r.git(&tree, &["push", "origin", "topic"]);
    r.advance();
    let before = r.snapshot(&tree);
    let primary = r.snapshot(&r.main);
    let trace = r.root.join("trace");
    let marker = r.install_fsmonitor();
    for (args, code, status) in [
        (
            vec!["sync", ".", "--main", "--dry-run", "--rebase", "--json"],
            4,
            "refused",
        ),
        (
            vec![
                "sync",
                ".",
                "--main",
                "--dry-run",
                "--rebase",
                "--rewrite-published",
                "--json",
            ],
            0,
            "updated",
        ),
    ] {
        let out = r
            .command(env!("CARGO_BIN_EXE_rabi"), &tree, &args)
            .env("GIT_TRACE", &trace)
            .output()
            .unwrap();
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(out.status.code(), Some(code), "{doc}");
        assert_eq!(doc["data"]["plan"], true);
        assert_eq!(doc["data"]["items"][0]["status"], status);
        assert_eq!(doc["data"]["items"][0]["rewrotePublished"], false);
        assert_eq!(doc["warnings"], json!([]));
        assert!(!marker.exists());
        let traced = fs::read_to_string(&trace).unwrap();
        assert_eq!(
            traced
                .matches("fetch --prune --no-auto-maintenance --no-recurse-submodules -- origin")
                .count(),
            1
        );
        fs::remove_file(&trace).unwrap();
    }
    r.git(&r.main, &["config", "core.fsmonitor", "false"]);
    assert_eq!(r.snapshot(&tree), before);
    assert_eq!(r.snapshot(&r.main), primary);
}

#[test]
fn p8_main_rechecks_primary_branch_after_fetch() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    let before = r.git(&r.main, &["rev-parse", "main"]);
    r.advance();
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|path| path.is_file())
        .unwrap();
    let bin = r.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, "#!/bin/sh\n\"$REAL_GIT\" \"$@\" || exit $?\nfor arg do\n  if [ \"$arg\" = fetch ]; then env -u GIT_DIR -u GIT_COMMON_DIR -u GIT_OBJECT_DIRECTORY \"$REAL_GIT\" checkout -b moved-during-fetch || exit $?; fi\ndone\n").unwrap();
    fs::set_permissions(wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let out = r
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &r.main,
            &["sync", "--main", "--json"],
        )
        .env("REAL_GIT", real_git)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{doc}");
    assert_eq!(doc["data"]["main"]["status"], "not-default-branch");
    assert_eq!(r.git(&r.main, &["rev-parse", "main"]), before);
    assert_eq!(r.git(&r.main, &["rev-parse", "HEAD"]), before);
}

#[test]
fn p8_invalid_scopes_and_strategies_are_usage_without_repository_context() {
    let r = Repo::new();
    for args in [
        vec!["--all", "topic"],
        vec!["--auto", "--rebase"],
        vec!["--auto", "--merge"],
        vec!["--rebase", "--merge"],
    ] {
        let doc = r.sync(&r.root, &args, 2);
        assert_eq!(doc["error"]["code"], "usage");
    }
}

#[test]
fn fetch_refuses_explicit_tag_destinations() {
    for dry_run in [true, false] {
        for destination in ["refs/tags/cache/*", "refs/tags/cache/release"] {
            let r = Repo::new();
            let source = if destination.contains('*') {
                "refs/heads/*"
            } else {
                "refs/heads/release"
            };
            let spec = format!("+{source}:{destination}");
            r.git(&r.main, &["branch", "release"]);
            r.git(
                &r.main,
                &[
                    "symbolic-ref",
                    "refs/tags/cache/release",
                    "refs/heads/release",
                ],
            );
            r.git(&r.main, &["config", "--add", "remote.origin.fetch", &spec]);
            r.advance();
            r.git(&r.upstream, &["push", "origin", "HEAD:release"]);
            assert_fetch_refused(&r, &r.main, dry_run, &[], &["origin", &spec, "Configure"]);
        }
    }
}

#[test]
fn fetch_refuses_visible_tag_symrefs_without_explicit_tag_refspecs() {
    for dry_run in [true, false] {
        for target in [
            "refs/heads/release",
            "refs/tags/existing",
            "refs/cache/existing",
        ] {
            let r = Repo::new();
            r.git(&r.main, &["update-ref", target, "HEAD"]);
            r.git(&r.main, &["symbolic-ref", "refs/tags/alias", target]);
            r.advance();
            r.git(&r.upstream, &["tag", "alias"]);
            r.git(&r.upstream, &["push", "origin", "refs/tags/alias"]);
            assert_fetch_refused(
                &r,
                &r.main,
                dry_run,
                &[],
                &["refs/tags/alias", target, "Repair"],
            );
        }
    }
}

#[test]
fn unknown_planned_tips_propagate_through_workspace_dependencies() {
    for strategy in ["--rebase", "--merge"] {
        let r = Repo::new();
        let a = r.create("a");
        r.write_commit(&a, "a-file", "a\n");
        let b = r.create("b");
        r.git(&b, &["reset", "--hard", "a"]);
        assert!(r.rabi(&b, &["set", "--base", "a"]).status.success());
        r.write_commit(&b, "b-file", "b\n");
        let c = r.create("c");
        r.git(&c, &["reset", "--hard", "b"]);
        assert!(r.rabi(&c, &["set", "--base", "b"]).status.success());
        r.write_commit(&c, "c-file", "c\n");
        let before = [r.snapshot(&a), r.snapshot(&b), r.snapshot(&c)];
        let refs = r.git(&r.main, &["for-each-ref", "refs/heads"]);
        let reflogs = file_contents(&r.main.join(".git/logs"));
        let records = file_contents(&r.main.join(".git/rabi/workspaces"));
        r.advance();
        let plan = r.sync(&r.main, &["a", "b", "c", strategy, "--dry-run"], 0);
        assert_eq!([r.snapshot(&a), r.snapshot(&b), r.snapshot(&c)], before);
        assert_eq!(r.git(&r.main, &["for-each-ref", "refs/heads"]), refs);
        // Fetch may update tracking reflogs, but never local branch reflogs.
        let after_logs = file_contents(&r.main.join(".git/logs"));
        for (path, content) in &reflogs {
            if path.contains("refs/heads/") || path == "HEAD" {
                assert_eq!(&after_logs[path], content);
            }
        }
        assert_eq!(file_contents(&r.main.join(".git/rabi/workspaces")), records);
        for i in 0..3 {
            let item = &plan["data"]["items"][i];
            assert_eq!(item["status"], "updated", "{plan}");
            assert!(item["after"].is_null(), "{plan}");
            if i > 0 {
                let dependency = if i == 1 {
                    "refs/heads/a"
                } else {
                    "refs/heads/b"
                };
                assert!(
                    item["detail"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("depends on planned update of {dependency}")),
                    "{plan}"
                );
                assert!(item["strategy"].is_null(), "{plan}");
            }
        }
        let actual = r.sync(&r.main, &["a", "b", "c", strategy], 0);
        for i in 0..3 {
            assert_eq!(actual["data"]["items"][i]["status"], "updated", "{actual}");
            assert_ne!(actual["data"]["items"][i]["after"], before[i]["head"]);
        }
    }
}

#[test]
fn current_branch_fetch_destination_is_not_publication_evidence() {
    let r = Repo::new();
    let tree = r.create("topic");
    r.write_commit(&tree, "own", "unpublished\n");
    r.git(&r.main, &["remote", "add", "archive", "unused"]);
    r.git(
        &r.main,
        &[
            "config",
            "remote.archive.fetch",
            "+refs/heads/topic:refs/heads/topic",
        ],
    );
    r.advance();
    let doc = r.sync(&tree, &[], 0);
    assert_eq!(doc["data"]["items"][0]["strategy"], "rebase", "{doc}");
    assert_eq!(doc["data"]["items"][0]["status"], "updated", "{doc}");
}

#[test]
fn independent_local_upstream_is_publication_evidence() {
    for explicit in [true, false] {
        let r = Repo::new();
        let tree = r.create("topic");
        r.write_commit(&tree, "published", "published\n");
        r.git(&tree, &["branch", "published-copy"]);
        r.git(&tree, &["branch", "--set-upstream-to=published-copy"]);
        r.advance();
        let before = r.snapshot(&tree);
        let doc = r.sync(
            &tree,
            if explicit { &["--rebase"] } else { &[] },
            if explicit { 4 } else { 0 },
        );
        if explicit {
            assert_eq!(doc["data"]["items"][0]["code"], "published", "{doc}");
            assert_eq!(r.snapshot(&tree), before);
        } else {
            assert_eq!(doc["data"]["items"][0]["strategy"], "merge", "{doc}");
            r.git(
                &tree,
                &[
                    "merge-base",
                    "--is-ancestor",
                    before["head"].as_str().unwrap(),
                    "HEAD",
                ],
            );
        }
    }
}

#[test]
fn tag_symref_into_remote_tracking_namespace_is_allowed() {
    let r = Repo::new();
    r.git(
        &r.main,
        &[
            "symbolic-ref",
            "refs/tags/alias",
            "refs/remotes/origin/main",
        ],
    );
    r.advance();
    let plan = r.sync(&r.main, &["--dry-run"], 0);
    assert_eq!(plan["data"]["main"]["status"], "updated");
    assert_eq!(
        r.git(&r.main, &["symbolic-ref", "refs/tags/alias"]),
        "refs/remotes/origin/main"
    );
    let actual = r.sync(&r.main, &[], 0);
    assert_eq!(
        actual["data"]["main"]["after"],
        plan["data"]["main"]["after"]
    );
}
