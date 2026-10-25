//! Shared diagnostics and one JSON document per invocation.
use std::cell::RefCell;
use std::fmt;
use std::io::{self, Write};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, JsonErrorBody, Result};

// Keep stream state separate from the JSON envelope: diagnostics can be emitted
// while OUTPUT is borrowed, and begin() must never reopen a broken stream.
static STDOUT: Mutex<Stream> = Mutex::new(Stream::new());
static STDERR: Mutex<Stream> = Mutex::new(Stream::new());

struct Stream {
    closed: bool,
    error: Option<io::Error>,
}

impl Stream {
    const fn new() -> Self {
        Self {
            closed: false,
            error: None,
        }
    }

    fn write(&mut self, writer: &mut impl Write, args: fmt::Arguments<'_>) {
        if self.closed {
            return;
        }
        // Flush here as well: even output without a newline must observe EPIPE
        // now, rather than leaving a buffered write for process shutdown.
        if let Err(error) = writer.write_fmt(args).and_then(|()| writer.flush()) {
            self.closed = true;
            if error.kind() != io::ErrorKind::BrokenPipe {
                self.error = Some(error);
            }
        }
    }

    fn check(&mut self, name: &str) -> Result<()> {
        match self.error.take() {
            Some(error) => Err(Error::msg(format!("Failed to write {name}: {error}"))),
            None => Ok(()),
        }
    }
}

pub(crate) fn stdout(args: fmt::Arguments<'_>) {
    STDOUT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .write(&mut io::stdout().lock(), args);
}

/// Write a diagnostic, ignoring a closed reader and deferring other IO errors.
pub fn stderr(args: fmt::Arguments<'_>) {
    STDERR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .write(&mut io::stderr().lock(), args);
}

/// Called after command processing, so an output failure cannot interrupt a
/// mutation. Broken pipes leave the command's original exit status unchanged.
pub(crate) fn check_io() -> Result<()> {
    let stdout = STDOUT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .check("stdout");
    let stderr = check_stderr();
    stdout.and(stderr)
}

/// Check prompt output before reading input, leaving stdout errors deferred.
pub(crate) fn check_stderr() -> Result<()> {
    STDERR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .check("stderr")
}

macro_rules! outln {
    ($($arg:tt)*) => {
        $crate::output::stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}

macro_rules! errln {
    ($($arg:tt)*) => {
        $crate::output::stderr(format_args!("{}\n", format_args!($($arg)*)))
    };
}

pub(crate) use {errln, outln};

#[derive(Default)]
struct Output {
    command: String,
    json: bool,
    data: Option<Value>,
    warnings: Vec<Warning>,
    error: Option<JsonErrorBody>,
    reported_errors: Vec<String>,
}

#[derive(Serialize)]
struct Warning {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<String>,
}

thread_local! {
    static OUTPUT: RefCell<Output> = RefCell::new(Output::default());
}

pub(crate) fn begin(command: &str, json: bool) {
    OUTPUT.with_borrow_mut(|out| {
        *out = Output {
            command: command.into(),
            json,
            ..Output::default()
        }
    });
}

pub(crate) fn command_is_exec() -> bool {
    OUTPUT.with_borrow(|out| out.command == "exec")
}

pub(crate) fn json() -> bool {
    OUTPUT.with_borrow(|out| out.json)
}

#[cfg(test)]
pub(crate) fn test_data() -> Option<Value> {
    OUTPUT.with_borrow(|out| out.data.clone())
}

pub(crate) fn data(value: &impl Serialize) -> Result<i32> {
    let mut value = serde_json::to_value(value).map_err(|error| Error::msg(error.to_string()))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("version");
        object.remove("schema");
        if let Some(error) = object.remove("error").filter(|error| !error.is_null()) {
            let body =
                serde_json::from_value(error).map_err(|error| Error::msg(error.to_string()))?;
            failure(body);
        }
    }
    OUTPUT.with_borrow_mut(|out| out.data = Some(value));
    Ok(0)
}

pub(crate) fn app_version(version: String) -> Result<i32> {
    OUTPUT.with_borrow_mut(|out| {
        out.data =
            Some(serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "display": version}))
    });
    Ok(0)
}

pub(crate) fn failure(error: JsonErrorBody) {
    failure_with(error, |message| errln!("{message}"));
}

fn diagnostic_message(message: &str) -> &str {
    message
        .trim_start()
        .strip_prefix("Warning: ")
        .unwrap_or(message.trim_start())
}

fn failure_with(error: JsonErrorBody, report: impl FnOnce(&str)) {
    OUTPUT.with_borrow_mut(|out| {
        if !out.json && !out.reported_errors.contains(&error.message) {
            if !out.warnings.iter().any(|warning| {
                diagnostic_message(&warning.message) == diagnostic_message(&error.message)
            }) {
                let message = if error.code == "partial" {
                    format!("Warning: {}", error.message)
                } else {
                    error.message.clone()
                };
                // Outer commands may report opaque IO/Git errors containing paths.
                // Ambiguity reports escape each value where their list is built,
                // so their intentional line breaks can remain readable.
                if ((is_outer_command(&out.command) && error.code != "ambiguous")
                    || matches!(
                        out.command.as_str(),
                        "create" | "adopt" | "forget" | "setup" | "remove"
                    ))
                    // Partial errors contain an already shell-encoded retry
                    // command. Preserve it verbatim for execution in bash/zsh.
                    && !(matches!(out.command.as_str(), "create" | "remove") && error.code == "partial")
                {
                    report(&visible_text(&message));
                } else {
                    report(&message);
                }
            }
            out.reported_errors.push(error.message.clone());
        }
        out.error = Some(error);
    });
}

pub(crate) fn warning(code: &str, message: impl Into<String>) {
    let message = message.into();
    if code == "repo_discovery" {
        errln!("{}", visible_text(&message));
    } else {
        errln!("{message}");
    }
    OUTPUT.with_borrow_mut(|out| {
        out.warnings.push(Warning {
            code: code.into(),
            message,
            workspace: None,
        })
    });
}

pub(crate) fn workspace_warning(code: &str, message: &str, workspace: &str) {
    errln!("{}: {}", visible_text(workspace), message);
    OUTPUT.with_borrow_mut(|out| {
        out.warnings.push(Warning {
            code: code.into(),
            message: message.into(),
            workspace: Some(workspace.into()),
        })
    });
}

pub(crate) fn finish(mut exit: i32) -> Result<i32> {
    // Finalize diagnostics before committing the JSON envelope and its status.
    // Commands have finished mutating; only the envelope's own stdout write
    // remains to be checked by the caller after this function returns.
    if json() {
        if let Err(error) = check_io() {
            exit = error.exit_code();
            failure(error.to_json(exit).error);
        }
    }
    OUTPUT.with_borrow_mut(|out| {
        if !out.json {
            return Ok(exit);
        }
        if exit != 0 && out.error.is_none() {
            out.error = Some(
                Error::status(exit, "Command did not complete successfully")
                    .to_json(exit)
                    .error,
            );
        }
        if exit == crate::error::PARTIAL && out.command != "run" {
            if let Some(error) = &mut out.error {
                error.code = "partial".into();
            }
        }
        let document = serde_json::json!({
            "schema": crate::error::JSON_VERSION, "command": out.command, "ok": exit == 0,
            "data": out.data, "warnings": out.warnings, "error": out.error,
        });
        let text = serde_json::to_string_pretty(&document)
            .map_err(|error| Error::msg(error.to_string()))?;
        outln!("{text}");
        Ok(exit)
    })
}

fn is_outer_command(command: &str) -> bool {
    matches!(
        command.split_whitespace().next(),
        Some("repo" | "trust" | "config" | "help" | "--version" | "skill")
    )
}

/// Values in diagnostics that provide their own trusted multiline layout.
pub(crate) fn diagnostic_value(value: &str) -> String {
    if json() {
        value.to_owned()
    } else {
        visible_text(value)
    }
}

/// Escape untrusted values before interpolating them into terminal output.
pub(crate) fn visible_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if hides_the_prompt(ch) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(ch)));
        } else {
            out.push(ch);
        }
    }
    out
}

fn hides_the_prompt(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{GENERAL, PARTIAL, REFUSED};

    #[derive(Default)]
    struct Probe {
        bytes: Vec<u8>,
        writes: usize,
        flushes: usize,
        error: Option<io::ErrorKind>,
        fail_on_flush: bool,
    }

    impl Write for Probe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if !self.fail_on_flush {
                if let Some(kind) = self.error {
                    return Err(kind.into());
                }
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.fail_on_flush {
                if let Some(kind) = self.error {
                    return Err(kind.into());
                }
            }
            Ok(())
        }
    }

    #[test]
    fn broken_stream_is_never_written_or_flushed_again() {
        for fail_on_flush in [false, true] {
            let mut stream = Stream::new();
            let mut writer = Probe {
                error: Some(io::ErrorKind::BrokenPipe),
                fail_on_flush,
                ..Default::default()
            };
            stream.write(&mut writer, format_args!("first"));
            assert!(stream.check("stdout").is_ok());
            let calls = (writer.writes, writer.flushes);
            writer.error = None;
            stream.write(&mut writer, format_args!("second"));
            assert_eq!((writer.writes, writer.flushes), calls);
            assert!(stream.check("stdout").is_ok());
        }
    }

    #[test]
    fn other_write_and_flush_errors_are_reported_after_processing() {
        for fail_on_flush in [false, true] {
            let mut stream = Stream::new();
            let mut writer = Probe {
                error: Some(io::ErrorKind::PermissionDenied),
                fail_on_flush,
                ..Default::default()
            };
            stream.write(&mut writer, format_args!("first"));
            let calls = (writer.writes, writer.flushes);
            stream.write(&mut writer, format_args!("second"));
            assert_eq!((writer.writes, writer.flushes), calls);
            let error = stream.check("stderr").unwrap_err();
            assert!(error.to_string().starts_with("Failed to write stderr:"));
            assert_eq!(error.exit_code(), GENERAL);
        }
    }

    #[test]
    fn stderr_check_leaves_stdout_errors_for_command_completion() {
        // Inject only the pending error so concurrent tests can still write.
        STDOUT.lock().unwrap().error = Some(io::ErrorKind::PermissionDenied.into());

        let prompt_result = check_stderr();
        let completion_result = check_io();

        assert!(prompt_result.is_ok());
        let error = completion_result.unwrap_err();
        assert!(error.to_string().starts_with("Failed to write stdout:"));
        assert_eq!(error.exit_code(), GENERAL);
        assert!(check_io().is_ok());
    }

    #[test]
    fn closing_one_stream_preserves_the_other_streams_bytes_and_flushes() {
        let mut stdout = Stream::new();
        let mut stderr = Stream::new();
        let mut broken = Probe {
            error: Some(io::ErrorKind::BrokenPipe),
            ..Default::default()
        };
        let mut writer = Probe::default();
        stdout.write(&mut broken, format_args!("ignored"));
        stderr.write(&mut writer, format_args!("line {}\n", 1));
        stderr.write(&mut writer, format_args!("prompt: "));
        assert_eq!(writer.bytes, b"line 1\nprompt: ");
        assert_eq!(writer.flushes, 2);
        assert!(stdout.check("stdout").is_ok());
        assert!(stderr.check("stderr").is_ok());
    }

    #[test]
    fn recorded_failures_are_reported_once_in_text_and_retained_in_json() {
        for json in [false, true] {
            for exit in [GENERAL, PARTIAL, REFUSED] {
                begin("test", json);
                // An unrelated warning must not hide a later error.
                OUTPUT.with_borrow_mut(|out| {
                    out.warnings.push(Warning {
                        workspace: None,
                        code: "unrelated".into(),
                        message: "Warning: unrelated".into(),
                    })
                });
                let error = Error::status(exit, "injected failure").to_json(exit).error;
                let mut messages = Vec::new();
                for _ in 0..2 {
                    failure_with(error.clone(), |message| messages.push(message.to_string()));
                }
                assert_eq!(messages.len(), usize::from(!json));
                if !json {
                    assert!(messages[0].contains("injected failure"));
                }
                OUTPUT.with_borrow(|out| {
                    assert_eq!(out.error.as_ref().unwrap().message, "injected failure")
                });
            }
        }
    }

    #[test]
    fn an_already_reported_warning_is_not_printed_again_as_an_error() {
        begin("test", false);
        warning("setup", "Warning: setup failed");
        failure_with(
            Error::status(PARTIAL, "setup failed")
                .to_json(PARTIAL)
                .error,
            |_| panic!("the same diagnostic must not be printed twice"),
        );
        OUTPUT.with_borrow(|out| assert_eq!(out.error.as_ref().unwrap().code, "partial"));
    }
}
