//! Closed parent-provider boundary; no task-progress interpretation.
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const MODEL: &str = "gpt-6.1-sol";
const EFFORT: &str = "model_reasoning_effort=\"high\"";

mod claude;
mod cursor;
mod stream;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Provider {
    Codex,
    Cursor,
    Claude,
}

pub(crate) enum Session<'a> {
    New { requested: Option<&'a str> },
    Resume { id: &'a str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    Present,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StopPolicy {
    pub signal: i32,
    pub grace: Duration,
    pub force_signal: i32,
    pub term_grace: Option<Duration>,
}

pub(crate) struct Final {
    pub session_id: String,
    pub is_error: bool,
    pub insecure_tool_calls: u64,
    pub protocol_error: Option<String>,
    pub session_validated: bool,
}

impl FromStr for Provider {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        match name {
            "codex" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "claude" => Ok(Self::Claude),
            _ => Err(Error::msg(format!("Unknown run provider: {name}"))),
        }
    }
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Claude => "claude",
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Cursor => "cursor-agent",
            Self::Claude => "claude",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Claude => "Claude",
        }
    }

    /// Build argv and attach the saved prompt without launching a process.
    pub fn command(self, workspace: &Path, dir: &Path, session: Session<'_>) -> Result<Command> {
        match self {
            Self::Codex => {
                let mut command = Command::new(self.binary());
                command
                    .args(["exec", "--json", "-C"])
                    .arg(workspace)
                    .args(["--approve-for-me", "-m", MODEL, "-c", EFFORT])
                    .arg("-o")
                    .arg(dir.join("answer.md"));
                match session {
                    Session::New { requested } => {
                        // Codex allocates its own session id after launch.
                        let _ = requested;
                    }
                    Session::Resume { id } => {
                        command.args(["resume", id]);
                    }
                }
                command.arg("-").current_dir(workspace);
                command.stdin(Stdio::from(
                    fs::File::open(dir.join("prompt.md")).map_err(io_error)?,
                ));
                Ok(command)
            }
            Self::Cursor => cursor::command(workspace, dir, session),
            Self::Claude => claude::command(workspace, dir, session),
        }
    }

    pub fn observe(self, line: &str) -> Option<String> {
        match self {
            Self::Codex => thread_id(line),
            Self::Cursor => cursor::session_id(line),
            Self::Claude => claude::session_id(line),
        }
    }

    pub fn finalize(self, events: &Path) -> Result<Final> {
        match self {
            Self::Codex => {
                // Codex owns answer.md through -o; finalization only recovers
                // the first session id, leaving answer/error handling unchanged.
                for line in BufReader::new(fs::File::open(events).map_err(io_error)?).lines() {
                    let line = line.map_err(io_error)?;
                    if let Some(session_id) = self.observe(&line) {
                        return Ok(Final {
                            session_id,
                            is_error: false,
                            insecure_tool_calls: 0,
                            protocol_error: None,
                            session_validated: true,
                        });
                    }
                }
                Err(Error::msg("Codex did not report thread.started.thread_id"))
            }
            Self::Cursor => cursor::finalize(events),
            Self::Claude => claude::finalize(events),
        }
    }

    pub fn probe_session(self, id: &str, workspace: &Path) -> Result<Probe> {
        match self {
            Self::Codex => has_rollout(id).map(|present| {
                if present {
                    Probe::Present
                } else {
                    Probe::Missing
                }
            }),
            Self::Cursor => cursor::probe_session(id, workspace),
            Self::Claude => claude::probe_session(id),
        }
    }

    pub fn prepare_workspace(self, workspace: &Path, common_dir: &Path) -> Result<()> {
        match self {
            Self::Codex | Self::Claude => Ok(()),
            Self::Cursor => cursor::prepare_workspace(workspace, common_dir),
        }
    }

    pub fn stop_policy(self) -> StopPolicy {
        match self {
            Self::Codex => StopPolicy {
                signal: 15,
                grace: Duration::from_millis(200),
                force_signal: 9,
                term_grace: None,
            },
            Self::Claude => StopPolicy {
                signal: 2,
                grace: Duration::from_secs(2),
                force_signal: 9,
                term_grace: Some(Duration::from_secs(1)),
            },
            Self::Cursor => StopPolicy {
                signal: 2,
                grace: Duration::from_millis(200),
                force_signal: 9,
                term_grace: Some(Duration::from_millis(200)),
            },
        }
    }
}

fn resolve_binary_in(path: Option<&std::ffi::OsStr>, binaries: &[&str]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let directories: Vec<_> = std::env::split_paths(path?).collect();
    for binary in binaries {
        for directory in &directories {
            let candidate = directory.join(binary);
            if fs::metadata(&candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            {
                // Resolve relative PATH entries before setting the child's cwd.
                if let Ok(path) = std::path::absolute(candidate) {
                    return Some(path);
                }
            }
        }
    }
    None
}

pub(crate) struct Observation {
    pub session_id: Option<String>,
    pub error: Option<String>,
    pub validated: bool,
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
    provider: Provider,
    reader: BufReader<R>,
    partial: Vec<u8>,
    claude_validated: bool,
    claude_first_id: Option<String>,
}
impl SessionIdReader {
    pub fn open(provider: Provider, stdout: &Path) -> Result<Self> {
        Ok(Self::new(
            provider,
            fs::File::open(stdout).map_err(io_error)?,
        ))
    }
}
impl<R: Read> SessionIdReader<R> {
    pub fn new(provider: Provider, reader: R) -> Self {
        Self {
            provider,
            reader: BufReader::new(reader),
            partial: Vec::new(),
            claude_validated: false,
            claude_first_id: None,
        }
    }

    #[cfg(test)]
    pub fn poll(&mut self) -> Result<Option<String>> {
        Ok(self.poll_observation()?.and_then(|event| event.session_id))
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    pub fn poll_observation(&mut self) -> Result<Option<Observation>> {
        // Claude init events can include large metadata. Use the final-scan
        // bound for both a line and a poll so permission validation is prompt
        // without unbounded work; Codex/Cursor retain their original bounds.
        let line_limit = if self.provider == Provider::Claude {
            stream::FINAL_LINE_BYTES
        } else {
            EVENT_LINE_BYTES
        };
        let mut remaining = if self.provider == Provider::Claude {
            stream::FINAL_LINE_BYTES
        } else {
            EVENT_POLL_BYTES
        };
        while remaining > 0 {
            let available = self.reader.fill_buf().map_err(io_error)?;
            let available = &available[..available.len().min(remaining)];
            if available.is_empty() {
                return Ok(None);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |position| position + 1);
            if self.partial.len() + consumed > line_limit {
                return Err(Error::msg(format!(
                    "{} event exceeds the observation line limit",
                    self.provider.display_name()
                )));
            }
            self.partial.extend_from_slice(&available[..consumed]);
            self.reader.consume(consumed);
            remaining -= consumed;
            if newline.is_some() {
                let line =
                    std::str::from_utf8(&self.partial).map_err(|e| Error::msg(e.to_string()))?;
                let mut init = false;
                let (mut id, mode) = if self.provider == Provider::Claude {
                    let event: serde_json::Value = serde_json::from_str(line).map_err(|_| {
                        Error::msg("Claude emitted a malformed or truncated JSON event")
                    })?;
                    (
                        claude::event_session_id(&event),
                        claude::permission_mode_event(&event),
                    )
                } else {
                    (self.provider.observe(line), None)
                };
                let error = if self.provider == Provider::Claude {
                    match mode {
                        Some(Ok(())) => {
                            init = true;
                            self.claude_validated = true;
                            None
                        }
                        Some(Err(error)) => {
                            init = true;
                            self.claude_validated = false;
                            Some(error)
                        }
                        None => None,
                    }
                } else {
                    None
                };
                if self.provider == Provider::Claude {
                    if self.claude_first_id.is_none() {
                        self.claude_first_id = id.clone();
                    } else if !init {
                        // Claude repeats the session id on every event. Keep
                        // scanning init validation without rewriting state for
                        // every assistant/tool event; finalize checks result ids.
                        id = None;
                    }
                }
                self.partial.clear();
                if id.is_some() || error.is_some() || init {
                    return Ok(Some(Observation {
                        session_id: id,
                        error,
                        validated: self.provider != Provider::Claude || self.claude_validated,
                    }));
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

fn has_rollout(session: &str) -> Result<bool> {
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
    fn provider_name_round_trips_and_unknown_names_are_rejected() {
        assert_eq!("codex".parse::<Provider>().unwrap(), Provider::Codex);
        assert_eq!("cursor".parse::<Provider>().unwrap(), Provider::Cursor);
        assert_eq!(Provider::Cursor.name(), "cursor");
        assert_eq!(Provider::Cursor.display_name(), "Cursor");
        assert_eq!(
            serde_json::to_string(&Provider::Cursor).unwrap(),
            "\"cursor\""
        );
        assert_eq!(
            serde_json::from_str::<Provider>("\"cursor\"").unwrap(),
            Provider::Cursor
        );
        assert_eq!(Provider::Codex.name(), "codex");
        assert_eq!(Provider::Codex.binary(), "codex");
        assert_eq!(Provider::Codex.display_name(), "Codex");
        assert_eq!(
            serde_json::to_string(&Provider::Codex).unwrap(),
            "\"codex\""
        );
        assert_eq!(
            serde_json::from_str::<Provider>("\"codex\"").unwrap(),
            Provider::Codex
        );
        for name in ["", "Codex", "unknown"] {
            assert!(name.parse::<Provider>().is_err());
            assert!(serde_json::from_value::<Provider>(serde_json::json!(name)).is_err());
        }
    }

    #[test]
    fn codex_command_matches_existing_new_and_resume_argv_and_prompt_stdin() {
        use std::ffi::OsString;
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("rabi-command-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace 二");
        let dir = root.join("launch dir");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("prompt.md"), b"saved prompt\nwith a second line\n").unwrap();
        // Execution only verifies the configured stdin; argv is checked directly
        // against the literal pre-boundary invocation before adding a local PATH.
        fs::write(root.join("codex"), "#!/bin/sh\n/bin/cat\n").unwrap();
        fs::set_permissions(root.join("codex"), fs::Permissions::from_mode(0o700)).unwrap();
        for session in [
            Session::New { requested: None },
            Session::Resume { id: "thread-id" },
        ] {
            let mut expected: Vec<OsString> = ["exec", "--json", "-C"]
                .into_iter()
                .map(OsString::from)
                .collect();
            expected.push(workspace.as_os_str().to_owned());
            expected.extend(
                [
                    "--approve-for-me",
                    "-m",
                    "gpt-6.1-sol",
                    "-c",
                    "model_reasoning_effort=\"high\"",
                    "-o",
                ]
                .into_iter()
                .map(OsString::from),
            );
            expected.push(dir.join("answer.md").into_os_string());
            if matches!(session, Session::Resume { .. }) {
                expected.extend(["resume", "thread-id"].into_iter().map(OsString::from));
            }
            expected.push(OsString::from("-"));
            let mut command = Provider::Codex.command(&workspace, &dir, session).unwrap();
            assert_eq!(command.get_program(), "codex");
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            assert_eq!(command.get_current_dir(), Some(workspace.as_path()));
            assert_eq!(command.get_envs().count(), 0);
            // The already-open saved prompt must remain stdin even if its path
            // is replaced after command construction.
            fs::rename(dir.join("prompt.md"), dir.join("original-prompt.md")).unwrap();
            fs::write(dir.join("prompt.md"), b"replacement").unwrap();
            let output = command.env("PATH", &root).output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"saved prompt\nwith a second line\n");
            fs::rename(dir.join("original-prompt.md"), dir.join("prompt.md")).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_observes_only_nonempty_thread_started_ids() {
        let provider = Provider::Codex;
        assert_eq!(
            provider.observe(r#"{"type":"thread.started","thread_id":"session-二"}"#),
            Some("session-二".into())
        );
        for line in [
            "not json",
            r#"{"type":"thread.started","thread_id":""}"#,
            r#"{"type":"thread.started"}"#,
            r#"{"type":"thread.started","thread_id":42}"#,
            r#"{"type":"other","thread_id":"wrong"}"#,
        ] {
            assert_eq!(provider.observe(line), None, "{line}");
        }
    }

    #[test]
    fn codex_finalize_uses_first_id_and_leaves_the_answer_file_untouched() {
        let dir = std::env::temp_dir().join(format!("rabi-finalize-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let events = dir.join("events.jsonl");
        let answer = dir.join("answer.md");
        fs::write(&answer, b"answer from -o\n").unwrap();
        fs::write(
            &events,
            concat!(
                "not json\n",
                "{\"type\":\"thread.started\",\"thread_id\":\"\"}\n",
                "{\"type\":\"thread.started\",\"thread_id\":\"first\"}\n",
                "{\"type\":\"thread.started\",\"thread_id\":\"second\"}"
            ),
        )
        .unwrap();
        assert_eq!(
            Provider::Codex.finalize(&events).unwrap().session_id,
            "first"
        );
        assert_eq!(fs::read(&answer).unwrap(), b"answer from -o\n");
        fs::write(
            &events,
            r#"{"type":"thread.started","thread_id":"no-newline"}"#,
        )
        .unwrap();
        assert_eq!(
            Provider::Codex.finalize(&events).unwrap().session_id,
            "no-newline"
        );
        fs::write(&events, "not json\n").unwrap();
        assert_eq!(
            Provider::Codex.finalize(&events).err().unwrap().to_string(),
            "Codex did not report thread.started.thread_id"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_probe_uses_an_isolated_codex_home() {
        const CHILD: &str = "RABI_TEST_PROVIDER_PROBE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(
                Provider::Codex
                    .probe_session("present", Path::new("/ignored"))
                    .unwrap(),
                Probe::Present
            );
            assert_eq!(
                Provider::Codex
                    .probe_session("missing", Path::new("/ignored"))
                    .unwrap(),
                Probe::Missing
            );
            return;
        }
        let dir = std::env::temp_dir().join(format!("rabi-probe-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("sessions/2026/10/26")).unwrap();
        fs::write(
            dir.join("sessions/2026/10/26/rollout-test-present.jsonl"),
            "",
        )
        .unwrap();
        // A child test process avoids mutating environment shared by unit tests.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider::tests::codex_probe_uses_an_isolated_codex_home",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("CODEX_HOME", &dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_stop_policy_preserves_term_grace_and_kill() {
        assert_eq!(
            Provider::Codex.stop_policy(),
            StopPolicy {
                signal: 15,
                grace: Duration::from_millis(200),
                force_signal: 9,
                term_grace: None,
            }
        );
        assert_eq!(
            Provider::Cursor.stop_policy(),
            StopPolicy {
                signal: 2,
                grace: Duration::from_millis(200),
                force_signal: 9,
                term_grace: Some(Duration::from_millis(200)),
            }
        );
    }

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
        let mut reader = SessionIdReader::new(
            Provider::Codex,
            Endless {
                pattern: b"{\"type\":\"noise\"}\n",
                offset: 0,
            },
        );
        for _ in 0..3 {
            let before = reader.reader.get_ref().offset;
            assert_eq!(reader.poll().unwrap(), None);
            assert!(reader.reader.get_ref().offset - before <= EVENT_POLL_BYTES);
            assert!(reader.partial.len() <= EVENT_LINE_BYTES);
        }
        let mut reader = SessionIdReader::new(
            Provider::Codex,
            Endless {
                pattern: b"x",
                offset: 0,
            },
        );
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
        let mut reader = SessionIdReader::open(Provider::Codex, &path).unwrap();
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
