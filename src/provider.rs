//! CLI adapters: argv, session extraction and final-answer files only.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};
use crate::workflow::{Agent, Mode, Provider};

pub(crate) fn supported(provider: Provider) -> Result<()> {
    // TODO: The future Claude Code adapter must use auto mode for edit steps;
    // read-only remains unsupported until its enforcement is verified.
    match provider {
        Provider::Codex => Ok(()),
        Provider::Claude | Provider::Cursor => Err(Error::msg(format!(
            "Provider {provider:?} is not supported yet"
        ))),
    }
}

pub(crate) fn codex(
    agent: &Agent,
    mode: &Mode,
    workspace: &Path,
    dir: &Path,
    session: Option<&str>,
) -> Result<Command> {
    supported(agent.provider)?;
    let mut command = Command::new("codex");
    // Exec-level flags, including the mode flags, must precede resume.
    command.args(["exec", "--json", "-C"]);
    command.arg(workspace);
    match mode {
        Mode::ReadOnly => {
            command.args(["--sandbox", "read-only"]);
        }
        Mode::Edit => {
            // Auto mode lets agents commit through automatic approval review.
            // Explicit workspace-write keeps linked Git metadata read-only even
            // with --add-dir; --approve-for-me conflicts with --sandbox.
            command.arg("--approve-for-me");
        }
    }
    if let Some(model) = &agent.model {
        command.args(["-m", model]);
    }
    if let Some(effort) = &agent.effort {
        // JSON strings are also TOML basic strings, including escaped quotes.
        let quoted =
            serde_json::to_string(effort).map_err(|error| Error::msg(error.to_string()))?;
        command.args(["-c", &format!("model_reasoning_effort={quoted}")]);
    }
    command.arg("-o").arg(dir.join("answer.md"));
    if let Some(session) = session {
        command.args(["resume", session]);
    }
    command.arg("-").current_dir(workspace);
    command.stdin(Stdio::from(
        fs::File::open(dir.join("prompt.md")).map_err(|error| Error::msg(error.to_string()))?,
    ));
    Ok(command)
}

pub(crate) fn session_id(stdout: &Path) -> Result<String> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(stdout).map_err(|error| Error::msg(error.to_string()))?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| Error::msg(error.to_string()))?;
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
            if event["type"] == "thread.started" {
                if let Some(id) = event["thread_id"].as_str().filter(|id| !id.is_empty()) {
                    return Ok(id.to_owned());
                }
            }
        }
    }
    Err(Error::msg("Codex did not report thread.started.thread_id"))
}
