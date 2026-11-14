//! Fresh reviewer capabilities and bounded process lifetime.
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::boundary::{Boundary, Process, Signals};
use super::implement::Provider;

const TIMEOUT: Duration = Duration::from_secs(10 * 60);
const ANSWER_BYTES: u64 = 64 * 1024;

fn command(
    provider: Provider,
    binary: &Path,
    workspace: &Path,
    answer: &Path,
    model: Option<&str>,
) -> Command {
    let mut command = Command::new(binary);
    match provider {
        Provider::Codex => {
            command
                .args([
                    "exec",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--sandbox",
                    "read-only",
                    "-c",
                    "approval_policy=\"never\"",
                    "-c",
                    "sandbox_workspace_write.network_access=false",
                    "-c",
                    "web_search=\"disabled\"",
                    "-c",
                    "shell_environment_policy.inherit=\"all\"",
                    "-c",
                    "shell_environment_policy.include_only=[]",
                    "-c",
                    "shell_environment_policy.set={}",
                    "--output-last-message",
                ])
                .arg(answer);
        }
        Provider::Claude => {
            command.args([
                "-p",
                "--permission-mode",
                "plan",
                "--permission-prompts",
                "none",
                "--tools",
                "Read,Grep,Glob",
                "--allowedTools",
                "Read,Grep,Glob",
                "--disallowedTools",
                "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch",
                "--safe-mode",
                "--output-format",
                "text",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--setting-sources",
                "",
                "--settings",
                "{\"disableAllHooks\":true}",
                "--disable-slash-commands",
                "--no-session-persistence",
            ]);
        }
    }
    if let Some(model) = model {
        command.arg(format!("--model={model}"));
    }
    if matches!(provider, Provider::Codex) {
        command.arg("-");
    }
    command
        .current_dir(workspace)
        .env("PWD", workspace)
        .env_remove("OLDPWD")
        .stderr(Stdio::null())
        .process_group(0);
    for key in [
        "RABI_REPO_PATH",
        "RABI_WORKSPACE_PATH",
        "RABI_WORKSPACE",
        "RABI_BIN",
    ] {
        command.env_remove(key);
    }
    command
}

pub(super) fn run(
    provider: Provider,
    workspace: &Path,
    prompt: &str,
    model: Option<&str>,
) -> Result<String, &'static str> {
    let name = match provider {
        Provider::Codex => "codex",
        Provider::Claude => "claude",
    };
    let binary = crate::provider::resolve_binary_in(std::env::var_os("PATH").as_deref(), &[name])
        .ok_or("review:provider-not-found")?;
    let boundary = Boundary::new().map_err(|_| "review:temporary-directory")?;
    let signals = Signals::install().map_err(|_| "review:signal-handler")?;
    let input = boundary.path().join("prompt.txt");
    fs::write(&input, prompt).map_err(|_| "review:prompt-write")?;
    let answer = boundary.path().join("answer.json");
    let mut command = command(provider, &binary, workspace, &answer, model);
    command.stdin(File::open(input).map_err(|_| "review:prompt-read")?);
    match provider {
        Provider::Codex => {
            command.stdout(Stdio::null());
        }
        Provider::Claude => {
            command.stdout(File::create(&answer).map_err(|_| "review:answer-write")?);
        }
    }
    boundary.prepare(&mut command);
    // A read-only git command may invoke configured external programs. Keep the
    // reviewer on disposable local metadata, without global hooks or drivers.
    let mut parameters = command
        .get_envs()
        .find(|(key, _)| *key == "GIT_CONFIG_PARAMETERS")
        .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
        .unwrap_or_default();
    parameters.push(" 'core.hooksPath=/dev/null' 'core.fsmonitor=false'");
    command
        .env("GIT_CONFIG_PARAMETERS", parameters)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let process = Process(command.spawn().map_err(|_| "review:launch-failed")?);
    let timeout = std::env::var("RABI_TEST_REVIEW_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(Duration::from_millis)
        .unwrap_or(TIMEOUT)
        .min(TIMEOUT);
    let started = Instant::now();
    loop {
        if signals.interrupted() {
            return Err("review:interrupted");
        }
        if started.elapsed() >= timeout {
            return Err("review:timeout");
        }
        if fs::metadata(&answer).is_ok_and(|metadata| metadata.len() > ANSWER_BYTES) {
            return Err("review:answer-too-large");
        }
        if let Some(code) = process.exit_code().map_err(|_| "review:wait-failed")? {
            if code != 0 {
                return Err("review:non-zero-exit");
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(process);
    let mut bytes = Vec::new();
    File::open(answer)
        .map_err(|_| "review:missing-answer")?
        .take(ANSWER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "review:answer-read")?;
    if bytes.len() as u64 > ANSWER_BYTES {
        return Err("review:answer-too-large");
    }
    String::from_utf8(bytes).map_err(|_| "review:invalid-answer-encoding")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_table_is_explicit() {
        for provider in [Provider::Codex, Provider::Claude] {
            let command = command(
                provider,
                Path::new("provider"),
                Path::new("checkout"),
                Path::new("answer"),
                Some("review-model"),
            );
            let args: Vec<_> = command
                .get_args()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            assert!(args.contains(&"--model=review-model".into()));
            assert!(!args.iter().any(|v| v == "--add-dir" || v == "--resume"));
            if matches!(provider, Provider::Codex) {
                assert!(args.windows(2).any(|a| a == ["--sandbox", "read-only"]));
                assert!(args.contains(&"approval_policy=\"never\"".into()));
                assert!(args.contains(&"sandbox_workspace_write.network_access=false".into()));
            } else {
                assert!(args.windows(2).any(|a| a == ["--permission-mode", "plan"]));
                for flag in ["--tools", "--allowedTools"] {
                    assert!(args.windows(2).any(|a| a == [flag, "Read,Grep,Glob"]));
                }
                assert!(args.windows(2).any(|a| a
                    == [
                        "--disallowedTools",
                        "Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch"
                    ]));
                assert!(!args.iter().any(|arg| arg.contains("Bash(")));
            }
        }
    }
}
