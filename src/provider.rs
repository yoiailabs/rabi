//! Codex invocation and protocol observations; no task-progress interpretation.
use std::fs;
use std::io::{BufRead, BufReader, Read};
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

/// Tail stdout with bounded work and memory, leaving incomplete JSON for later polls.
const EVENT_POLL_BYTES: usize = 64 * 1024;
const EVENT_LINE_BYTES: usize = 64 * 1024;

pub(crate) struct SessionIdReader<R = fs::File> {
    reader: BufReader<R>,
    partial: Vec<u8>,
}
impl SessionIdReader {
    pub fn open(stdout: &Path) -> Result<Self> {
        Ok(Self::new(fs::File::open(stdout).map_err(io_error)?))
    }
}
impl<R: Read> SessionIdReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            partial: Vec::new(),
        }
    }

    pub fn poll(&mut self) -> Result<Option<String>> {
        let mut remaining = EVENT_POLL_BYTES;
        while remaining > 0 {
            let available = self.reader.fill_buf().map_err(io_error)?;
            let available = &available[..available.len().min(remaining)];
            if available.is_empty() {
                return Ok(None);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |position| position + 1);
            if self.partial.len() + consumed > EVENT_LINE_BYTES {
                // An oversized line could contain the first thread id. Defer
                // to the final scan rather than trusting a later event.
                return Err(Error::msg("Codex event exceeds the observation line limit"));
            }
            self.partial.extend_from_slice(&available[..consumed]);
            self.reader.consume(consumed);
            remaining -= consumed;
            if newline.is_some() {
                let line =
                    std::str::from_utf8(&self.partial).map_err(|e| Error::msg(e.to_string()))?;
                let id = thread_id(line);
                self.partial.clear();
                if id.is_some() {
                    return Ok(id);
                }
            }
        }
        Ok(None)
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
    fn session_reader_bounds_each_poll_and_rejects_overlong_lines() {
        struct Endless {
            pattern: &'static [u8],
            offset: usize,
        }
        impl Read for Endless {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                for byte in bytes.iter_mut() {
                    *byte = self.pattern[self.offset % self.pattern.len()];
                    self.offset += 1;
                }
                Ok(bytes.len())
            }
        }
        let mut reader = SessionIdReader::new(Endless {
            pattern: b"{\"type\":\"noise\"}\n",
            offset: 0,
        });
        for _ in 0..3 {
            let before = reader.reader.get_ref().offset;
            assert_eq!(reader.poll().unwrap(), None);
            assert!(reader.reader.get_ref().offset - before <= EVENT_POLL_BYTES);
            assert!(reader.partial.len() <= EVENT_LINE_BYTES);
        }
        let mut reader = SessionIdReader::new(Endless {
            pattern: b"x",
            offset: 0,
        });
        assert_eq!(reader.poll().unwrap(), None);
        assert!(reader.poll().is_err());
        assert!(reader.partial.len() <= EVENT_LINE_BYTES);
        assert!(reader.reader.get_ref().offset <= EVENT_POLL_BYTES * 2);
    }

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
