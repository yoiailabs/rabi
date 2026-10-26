//! Cursor's stream protocol and local session storage, isolated from Codex.
use super::{io_error, Final, Probe, Session};
use crate::error::{Error, Result};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

mod md5;

// Tool and result events can exceed the live observer's 64 KiB bound. Refuse
// oversized lines without allocating beyond this separate finalization bound.
#[cfg(test)]
use super::stream::FINAL_LINE_BYTES;
const SANDBOX: &[u8] = b"{\"networkPolicy\":{\"allow\":[\"github.com\",\"*.github.com\",\"*.githubusercontent.com\"]}}\n";

pub(super) fn command(workspace: &Path, dir: &Path, session: Session<'_>) -> Result<Command> {
    let mut command = Command::new(resolve_binary()?);
    command
        .args(["-p", "--output-format", "stream-json", "--workspace"])
        .arg(workspace)
        .args([
            "--trust",
            "--sandbox",
            "enabled",
            "--auto-review",
            "--model",
            "grok-4.7-high",
        ]);
    if let Session::Resume { id } = session {
        command.args(["--resume", id]);
    }
    command.current_dir(workspace).stdin(Stdio::from(
        fs::File::open(dir.join("prompt.md")).map_err(io_error)?,
    ));
    Ok(command)
}

fn resolve_binary() -> Result<PathBuf> {
    resolve_binary_in(std::env::var_os("PATH").as_deref()).ok_or_else(|| Error::Spawn {
        command: "cursor-agent".into(),
        details: "Cursor CLI was not found on PATH (tried cursor-agent and agent). Install Cursor CLI from https://cursor.com/cli, then run `cursor-agent login` (or `agent login`) before retrying.".into(),
    })
}

fn resolve_binary_in(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    super::resolve_binary_in(path, &["cursor-agent", "agent"])
}

pub(super) fn session_id(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("session_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

pub(super) fn finalize(events: &Path) -> Result<Final> {
    let mut scanner = super::stream::Scanner::open(events, "Cursor")?;
    let mut first_id: Option<String> = None;
    let mut last_result = None;
    let mut protocol_error = None;
    let mut insecure_tool_calls = 0_u64;
    while let Some(event) = scanner.next(&mut protocol_error)? {
        if first_id.is_none() {
            first_id = event["session_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
        }
        if event["type"] == "system" && event["subtype"] == "init" {
            if let Some(id) = event["session_id"].as_str().filter(|id| !id.is_empty()) {
                if first_id.as_deref() != Some(id) {
                    protocol_error = Some(
                        "Cursor init session id differs from the first observed session id".into(),
                    );
                }
            }
        }
        if event["type"] == "tool_call"
            && event["subtype"] == "completed"
            && event["tool_call"].as_object().is_some_and(|tools| {
                tools.values().any(|tool| {
                    tool["args"]["requestedSandboxPolicy"]["type"] == "TYPE_INSECURE_NONE"
                })
            })
        {
            insecure_tool_calls = insecure_tool_calls.saturating_add(1);
        }
        if event["type"] == "result" {
            last_result = Some(event);
        }
    }
    let mut finalized = Final {
        session_id: first_id.unwrap_or_default(),
        is_error: false,
        insecure_tool_calls,
        protocol_error,
        session_validated: true,
    };
    let Some(result) = last_result else {
        finalized
            .protocol_error
            .get_or_insert_with(|| "Cursor did not report a result event".into());
        return Ok(finalized);
    };
    let answer = result["result"].as_str();
    let is_error = result["is_error"].as_bool();
    let result_id = result["session_id"].as_str().filter(|id| !id.is_empty());
    if let (Some(answer), Some(is_error), Some(result_id)) = (answer, is_error, result_id) {
        finalized.is_error = is_error;
        if finalized.session_id != result_id {
            finalized.protocol_error = Some(format!(
                "Cursor result session id ({result_id}) differs from the observed session id ({})",
                finalized.session_id
            ));
        }
        if finalized.protocol_error.is_none() {
            let answer_path = events
                .parent()
                .ok_or_else(|| Error::msg("Cursor events have no parent directory"))?
                .join("answer.md");
            if let Err(error) = crate::atomic::write(&answer_path, answer.as_bytes()) {
                finalized.protocol_error = Some(format!("Failed to write Cursor answer: {error}"));
            }
        }
    } else {
        finalized.protocol_error = Some(
            "Cursor reported a malformed result event (expected result, is_error and session_id)"
                .into(),
        );
    }
    Ok(finalized)
}

pub(super) fn probe_session(id: &str, workspace: &Path) -> Result<Probe> {
    let home = std::env::var_os("HOME").ok_or_else(|| Error::msg("HOME is not set"))?;
    Ok(probe_in(&PathBuf::from(home), id, workspace))
}

fn probe_in(home: &Path, id: &str, workspace: &Path) -> Probe {
    // A stored session id must remain one path component, even if state was
    // edited. Do not permit it to escape Cursor's workspace-specific directory.
    if id.is_empty()
        || !workspace.is_absolute()
        || !matches!(
            Path::new(id).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
    {
        return Probe::Missing;
    }
    let path = home
        .join(".cursor/chats")
        .join(md5::hex(workspace.as_os_str().as_bytes()))
        .join(id)
        .join("meta.json");
    let present = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|meta| meta["hasConversation"] == true);
    if present {
        Probe::Present
    } else {
        Probe::Missing
    }
}

pub(super) fn prepare_workspace(workspace: &Path, common_dir: &Path) -> Result<()> {
    // Never follow a checkout-controlled .cursor symlink or overwrite existing
    // sandbox content, including dangling symlinks and directories.
    // These checks do not prevent concurrent same-uid symlink replacement between
    // checking and opening .cursor, info, or info/exclude.
    let cursor = workspace.join(".cursor");
    match fs::create_dir(&cursor) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error)),
    }
    let metadata = fs::symlink_metadata(&cursor).map_err(io_error)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(cursor.join("sandbox.json"))
        {
            Ok(mut file) => file.write_all(SANDBOX).map_err(io_error)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    let info = common_dir.join("info");
    match fs::create_dir(&info) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(error)),
    }
    if !fs::symlink_metadata(&info).map_err(io_error)?.is_dir()
        || fs::symlink_metadata(&info)
            .map_err(io_error)?
            .file_type()
            .is_symlink()
    {
        return Err(Error::msg(
            "Refusing to write Cursor exclusion through a non-directory or symlink info path",
        ));
    }
    let exclude = info.join("exclude");
    match fs::symlink_metadata(&exclude) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(Error::msg(
                "Refusing to replace a non-file or symlink info/exclude",
            ))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let bytes = match fs::read(&exclude) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(io_error(error)),
    };
    if bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line == b".cursor/sandbox.json" || line == b"/.cursor/sandbox.json"
    }) {
        return Ok(());
    }
    // Append to preserve existing file ownership/mode and unrelated bytes.
    // Concurrent launches in different workspaces may append duplicate harmless
    // patterns; sequential preparation never adds the same pattern twice.
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .map_err(io_error)?;
    let suffix: &[u8] = if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        b"\n/.cursor/sandbox.json\n"
    } else {
        b"/.cursor/sandbox.json\n"
    };
    file.write_all(suffix).map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rabi-cursor-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        dir
    }
    fn finish(dir: &Path, text: &str) -> Final {
        fs::write(dir.join("events.jsonl"), text).unwrap();
        finalize(&dir.join("events.jsonl")).unwrap()
    }
    fn result(answer: &str, error: bool, id: &str) -> String {
        serde_json::json!({"type":"result","session_id":id,"is_error":error,"result":answer})
            .to_string()
    }

    #[test]
    fn binary_preference_checks_all_path_entries_before_fallback() {
        let dir = temp_dir();
        let second = dir.join("second");
        fs::create_dir(&second).unwrap();
        fs::write(dir.join("agent"), "fallback").unwrap();
        fs::set_permissions(dir.join("agent"), fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths([&dir, &second]).unwrap();
        assert_eq!(resolve_binary_in(Some(&path)), Some(dir.join("agent")));
        fs::write(second.join("cursor-agent"), "preferred").unwrap();
        assert_eq!(
            resolve_binary_in(Some(&path)),
            Some(dir.join("agent")),
            "nonexecutables are ignored"
        );
        fs::set_permissions(
            second.join("cursor-agent"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert_eq!(
            resolve_binary_in(Some(&path)),
            Some(second.join("cursor-agent"))
        );
        assert!(resolve_binary_in(None).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn last_result_and_no_newline_save_exact_answer_and_is_error() {
        let dir = temp_dir();
        let events = format!(
            "{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"id\"}}\n{}\n{}",
            result("discard", false, "id"),
            result("last\n二", true, "id")
        );
        let finalized = finish(&dir, &events);
        assert_eq!(finalized.session_id, "id");
        assert!(finalized.is_error);
        assert!(finalized.protocol_error.is_none());
        assert_eq!(
            fs::read(dir.join("answer.md")).unwrap(),
            "last\n二".as_bytes()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finalization_bounds_large_lines_and_rejects_truncation_and_bad_last_result() {
        let dir = temp_dir();
        let answer = "x".repeat(128 * 1024);
        assert!(finish(&dir, &result(&answer, false, "id"))
            .protocol_error
            .is_none());
        assert_eq!(fs::read(dir.join("answer.md")).unwrap().len(), answer.len());
        for text in [
            String::new(),
            format!("{}\n{{\"type\":\"result\"", result("old", false, "id")),
            format!(
                "{}\n{{\"type\":\"result\",\"result\":5}}",
                result("old", false, "id")
            ),
            format!(
                "{{\"session_id\":\"init\"}}\n{}",
                result("wrong", false, "result")
            ),
            result(&"x".repeat(FINAL_LINE_BYTES), false, "id"),
        ] {
            fs::remove_file(dir.join("answer.md")).ok();
            assert!(finish(&dir, &text).protocol_error.is_some());
            assert!(!dir.join("answer.md").exists());
        }
        fs::create_dir(dir.join("answer.md")).unwrap();
        fs::write(dir.join("events.jsonl"), result("cannot save", false, "id")).unwrap();
        assert!(finalize(&dir.join("events.jsonl"))
            .unwrap()
            .protocol_error
            .unwrap()
            .contains("Failed to write Cursor answer"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_counts_completed_policy_requests_even_when_protocol_fails() {
        let dir = temp_dir();
        let call = serde_json::json!({"type":"tool_call","subtype":"completed","tool_call":{"shellToolCall":{"args":{"requestedSandboxPolicy":{"type":"TYPE_INSECURE_NONE"}},"result":{"rejected":{}}}}});
        let started = serde_json::json!({"type":"tool_call","subtype":"started","tool_call":{"shellToolCall":{"args":{"requestedSandboxPolicy":{"type":"TYPE_INSECURE_NONE"}}}}});
        let finalized = finish(&dir, &format!("{call}\n{started}\n{call}\n"));
        assert_eq!(finalized.insecure_tool_calls, 2);
        assert!(finalized.protocol_error.is_some());
        fs::create_dir(dir.join("answer.md")).unwrap();
        let finalized = finish(&dir, &format!("{call}\n{}", result("answer", false, "id")));
        assert_eq!(finalized.insecure_tool_calls, 1);
        assert!(finalized
            .protocol_error
            .unwrap()
            .contains("Failed to write Cursor answer"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn probe_requires_conversation_and_workspace_hash_and_safe_id() {
        let home = temp_dir();
        let workspace = Path::new("/absolute/workspace 二");
        let meta = home
            .join(".cursor/chats")
            .join(md5::hex(workspace.as_os_str().as_bytes()))
            .join("id/meta.json");
        fs::create_dir_all(meta.parent().unwrap()).unwrap();
        for (content, expected) in [
            ("{}", Probe::Missing),
            ("bad", Probe::Missing),
            ("{\"hasConversation\":false}", Probe::Missing),
            ("{\"hasConversation\":true}", Probe::Present),
        ] {
            fs::write(&meta, content).unwrap();
            assert_eq!(probe_in(&home, "id", workspace), expected);
        }
        assert_eq!(
            probe_in(&home, "id", Path::new("/different")),
            Probe::Missing
        );
        assert_eq!(probe_in(&home, "unknown-uuid", workspace), Probe::Missing);
        assert_eq!(probe_in(&home, "../../id", workspace), Probe::Missing);
        assert_eq!(probe_in(&home, "id", Path::new("relative")), Probe::Missing);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn sandbox_preparation_is_idempotent_and_preserves_existing_bytes_and_modes() {
        let dir = temp_dir();
        let workspace = dir.join("ws");
        let common = dir.join("git");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir_all(common.join("info")).unwrap();
        let exclude = common.join("info/exclude");
        fs::write(&exclude, b"# existing\xff").unwrap();
        fs::set_permissions(&exclude, fs::Permissions::from_mode(0o640)).unwrap();
        prepare_workspace(&workspace, &common).unwrap();
        assert_eq!(
            fs::read(workspace.join(".cursor/sandbox.json")).unwrap(),
            SANDBOX
        );
        let bytes = fs::read(&exclude).unwrap();
        assert_eq!(bytes, b"# existing\xff\n/.cursor/sandbox.json\n");
        assert_eq!(
            fs::metadata(&exclude).unwrap().permissions().mode() & 0o777,
            0o640
        );
        prepare_workspace(&workspace, &common).unwrap();
        assert_eq!(fs::read(&exclude).unwrap(), bytes);
        fs::write(workspace.join(".cursor/sandbox.json"), "user content").unwrap();
        prepare_workspace(&workspace, &common).unwrap();
        assert_eq!(
            fs::read(workspace.join(".cursor/sandbox.json")).unwrap(),
            b"user content"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sandbox_and_cursor_symlinks_are_untouched_and_exclude_symlinks_refused() {
        let dir = temp_dir();
        let workspace = dir.join("ws");
        let common = dir.join("git");
        let outside = dir.join("outside");
        fs::create_dir_all(workspace.join(".cursor")).unwrap();
        fs::create_dir(&common).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(
            outside.join("missing"),
            workspace.join(".cursor/sandbox.json"),
        )
        .unwrap();
        prepare_workspace(&workspace, &common).unwrap();
        assert!(!outside.join("missing").exists());
        assert!(fs::symlink_metadata(workspace.join(".cursor/sandbox.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_file(workspace.join(".cursor/sandbox.json")).unwrap();
        fs::remove_dir(workspace.join(".cursor")).unwrap();
        symlink(&outside, workspace.join(".cursor")).unwrap();
        prepare_workspace(&workspace, &common).unwrap();
        assert!(!outside.join("sandbox.json").exists());
        fs::remove_file(common.join("info/exclude")).unwrap();
        fs::write(outside.join("exclude"), "untouched").unwrap();
        symlink(outside.join("exclude"), common.join("info/exclude")).unwrap();
        assert!(prepare_workspace(&workspace, &common).is_err());
        assert_eq!(fs::read(outside.join("exclude")).unwrap(), b"untouched");
        fs::remove_dir_all(dir).unwrap();
    }
}
