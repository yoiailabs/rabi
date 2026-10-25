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
        if let Some(id) = thread_id(&line) {
            return Ok(id);
        }
    }
    Err(Error::msg("Codex did not report thread.started.thread_id"))
}

fn thread_id(line: &str) -> Option<String> {
    let event = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if event["type"] == "thread.started" {
        event["thread_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    } else {
        None
    }
}

/// Tail stdout without rereading prior events or parsing incomplete JSON lines.
pub(crate) struct SessionIdReader {
    reader: BufReader<fs::File>,
    partial: Vec<u8>,
}
impl SessionIdReader {
    pub fn open(stdout: &Path) -> Result<Self> {
        Ok(Self {
            reader: BufReader::new(fs::File::open(stdout).map_err(io_error)?),
            partial: Vec::new(),
        })
    }

    pub fn poll(&mut self) -> Result<Option<String>> {
        loop {
            let read = self
                .reader
                .read_until(b'\n', &mut self.partial)
                .map_err(io_error)?;
            if read == 0 || !self.partial.ends_with(b"\n") {
                return Ok(None);
            }
            let line = std::str::from_utf8(&self.partial).map_err(|e| Error::msg(e.to_string()))?;
            let id = thread_id(line);
            self.partial.clear();
            if id.is_some() {
                return Ok(id);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn session_reader_retains_partial_lines_and_reads_only_new_events() {
        let path = std::env::temp_dir().join(format!("rabi-events-{}", uuid::Uuid::new_v4()));
        let mut writer = fs::File::create(&path).unwrap();
        writer.write_all(b"not json\n{\"type\":\"thread.started\",\"thread_id\":\"\"}\n{\"type\":\"thread.started\",\"thread_id\":\"").unwrap();
        let mut reader = SessionIdReader::open(&path).unwrap();
        assert_eq!(reader.poll().unwrap(), None);
        assert_eq!(reader.poll().unwrap(), None);
        let id = "session-二";
        let bytes = id.as_bytes();
        writer.write_all(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(reader.poll().unwrap(), None);
        writer.write_all(&bytes[bytes.len() - 1..]).unwrap();
        writer.write_all(b"\"}\n").unwrap();
        assert_eq!(reader.poll().unwrap().as_deref(), Some(id));
        assert_eq!(reader.poll().unwrap(), None);
        writer
            .write_all(b"{\"type\":\"thread.started\",\"thread_id\":\"next\"}\n")
            .unwrap();
        assert_eq!(reader.poll().unwrap().as_deref(), Some("next"));
        fs::remove_file(path).unwrap();
    }
}
