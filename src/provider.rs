//! Codex invocation and protocol observations; no task-progress interpretation.
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

const MODEL: &str = "gpt-6.1-sol";
const EFFORT: &str = "model_reasoning_effort=\"high\"";

pub(crate) fn codex(workspace: &Path, dir: &Path, session: Option<&str>) -> Result<Command> {
    let mut command = Command::new("codex");
    command
        .args(["exec", "--json", "-C"])
        .arg(workspace)
        .args(["--approve-for-me", "-m", MODEL, "-c", EFFORT])
        .arg("-o")
        .arg(dir.join("answer.md"));
    if let Some(session) = session {
        command.args(["resume", session]);
    }
    command.arg("-").current_dir(workspace);
    command.stdin(Stdio::from(
        fs::File::open(dir.join("prompt.md")).map_err(io_error)?,
    ));
    Ok(command)
}

pub(crate) fn session_id(stdout: &Path) -> Result<String> {
    for line in BufReader::new(fs::File::open(stdout).map_err(io_error)?).lines() {
        let line = line.map_err(io_error)?;
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
            if event["type"] == "thread.started" {
                if let Some(id) = event["thread_id"].as_str().filter(|s| !s.is_empty()) {
                    return Ok(id.to_owned());
                }
            }
        }
    }
    Err(Error::msg("Codex did not report thread.started.thread_id"))
}

pub(crate) fn codex_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".codex"))
        .ok_or_else(|| Error::msg("HOME is not set"))
}

pub(crate) fn has_rollout(session: &str) -> Result<bool> {
    fn visit(dir: &Path, suffix: &str) -> Result<bool> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(io_error(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            let kind = entry.file_type().map_err(io_error)?;
            if kind.is_dir() && visit(&entry.path(), suffix)? {
                return Ok(true);
            }
            if kind.is_file() && entry.file_name().to_string_lossy().ends_with(suffix) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    visit(
        &codex_home()?.join("sessions"),
        &format!("-{session}.jsonl"),
    )
}

fn io_error(error: std::io::Error) -> Error {
    Error::msg(error.to_string())
}
