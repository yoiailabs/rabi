//! Claude Code's stream protocol and best-effort local transcript probe.
use super::{io_error, Final, Probe, Session};
use crate::error::{Error, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) fn command(
    workspace: &Path,
    dir: &Path,
    session: Session<'_>,
    memory: Option<&Path>,
) -> Result<Command> {
    let binary = super::resolve_binary_in(std::env::var_os("PATH").as_deref(), &["claude"])
        .ok_or_else(|| Error::Spawn {
            command: "claude".into(),
            details: "Claude Code CLI was not found on PATH. Install Claude Code, then run `claude` to log in before retrying.".into(),
        })?;
    command_with_binary(binary, workspace, dir, session, memory)
}

fn command_with_binary(
    binary: PathBuf,
    workspace: &Path,
    dir: &Path,
    session: Session<'_>,
    memory: Option<&Path>,
) -> Result<Command> {
    let mut command = Command::new(binary);
    command.args(["-p", "--output-format", "stream-json", "--verbose"]);
    match session {
        Session::New {
            requested: Some(id),
        } => {
            command.args(["--session-id", id]);
        }
        Session::New { requested: None } => {
            return Err(Error::msg(
                "Claude requires a requested session id for a new launch",
            ));
        }
        Session::Resume { id } => {
            command.args(["--resume", id]);
        }
    }
    command.args([
        "--permission-mode",
        "auto",
        "--permission-prompts",
        "none",
        "--model",
        "opus",
        "--effort",
        "high",
    ]);
    if let Some(memory) = memory {
        command.arg("--add-dir").arg(memory);
    }
    command.current_dir(workspace).stdin(Stdio::from(
        fs::File::open(dir.join("prompt.md")).map_err(io_error)?,
    ));
    Ok(command)
}

pub(super) fn session_id(line: &str) -> Option<String> {
    let event = serde_json::from_str::<Value>(line).ok()?;
    event_session_id(&event)
}

pub(super) fn event_session_id(event: &Value) -> Option<String> {
    event["session_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Only init reports the effective permission mode; every init must be auto.
#[cfg(test)]
pub(super) fn permission_mode(line: &str) -> Option<std::result::Result<(), String>> {
    let event = serde_json::from_str::<Value>(line).ok()?;
    permission_mode_event(&event)
}

pub(super) fn permission_mode_event(event: &Value) -> Option<std::result::Result<(), String>> {
    if event["type"] != "system" || event["subtype"] != "init" {
        return None;
    }
    if event["permissionMode"] == "auto" {
        return Some(Ok(()));
    }
    let mode = event["permissionMode"]
        .as_str()
        .unwrap_or("missing or invalid");
    Some(Err(format!("Claude started in permission mode `{mode}`, not `auto`; rabi will not run Claude without auto mode (check Claude settings such as disableAutoMode)")))
}

pub(super) fn finalize(events: &Path) -> Result<Final> {
    let mut scanner = super::stream::Scanner::open(events, "Claude")?;
    let mut finalized = Final {
        session_id: String::new(),
        is_error: false,
        insecure_tool_calls: 0,
        protocol_error: None,
        session_validated: false,
    };
    let mut last_result = None;
    let mut permission_error = None;
    let mut identity_consistent = true;
    while let Some(event) = scanner.next(&mut finalized.protocol_error)? {
        if let Some(id) = event["session_id"].as_str().filter(|id| !id.is_empty()) {
            if finalized.session_id.is_empty() {
                finalized.session_id = id.into();
            }
            if event["type"] == "system" && event["subtype"] == "init" && finalized.session_id != id
            {
                identity_consistent = false;
                finalized.protocol_error = Some(
                    "Claude init session id differs from the first observed session id".into(),
                );
            }
        }
        if let Some(mode) = permission_mode_event(&event) {
            match mode {
                Ok(()) => finalized.session_validated = true,
                Err(error) => {
                    permission_error = Some(error);
                }
            }
        }
        if event["type"] == "result" {
            // Identity checks apply even to malformed/null result payloads and
            // earlier results, so a later valid result cannot restore trust.
            if let Some(id) = event["session_id"].as_str().filter(|id| !id.is_empty()) {
                if finalized.session_id != id {
                    identity_consistent = false;
                    finalized.protocol_error.get_or_insert_with(|| format!(
                        "Claude result session id ({id}) differs from the observed session id ({})", finalized.session_id
                    ));
                }
            }
            last_result = Some(event);
        }
    }
    if let Some(error) = permission_error {
        finalized.session_validated = false;
        finalized.protocol_error = Some(error);
    } else if !finalized.session_validated {
        finalized.protocol_error.get_or_insert_with(|| {
            "Claude did not report an init event with permission mode `auto`".into()
        });
    }
    finalized.session_validated &= identity_consistent;
    let Some(result) = last_result else {
        finalized
            .protocol_error
            .get_or_insert_with(|| "Claude did not report a result event".into());
        return Ok(finalized);
    };
    let answer = result["result"].as_str();
    let is_error = result["is_error"].as_bool();
    let result_id = result["session_id"].as_str().filter(|id| !id.is_empty());
    if let (Some(answer), Some(is_error), Some(_)) = (answer, is_error, result_id) {
        finalized.is_error = is_error;
        if finalized.protocol_error.is_none() {
            let answer_path = events
                .parent()
                .ok_or_else(|| Error::msg("Claude events have no parent directory"))?
                .join("answer.md");
            if let Err(error) = crate::atomic::write(&answer_path, answer.as_bytes()) {
                finalized.protocol_error = Some(format!("Failed to write Claude answer: {error}"));
            }
        }
    } else {
        // Claude's graceful INT result can have result:null (or omit it).
        // Leave answer.md absent, matching Cursor's malformed-result handling;
        // the launch's interruption/timeout reason takes precedence afterwards.
        finalized.protocol_error.get_or_insert_with(|| "Claude reported a malformed result event (expected result, is_error and session_id)".into());
    }
    Ok(finalized)
}

pub(super) fn probe_session(id: &str) -> Result<Probe> {
    let config = config_dir(
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        std::env::var_os("HOME"),
    );
    Ok(config.map_or(Probe::Unknown, |config| probe_in(&config, id)))
}

fn config_dir(
    config: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    config
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".claude")))
}

fn valid_id(id: &str) -> bool {
    // Canonical UUIDs are one safe component. Reject separators and alternative
    // UUID spellings before looking at the provider's internal storage.
    uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
}

fn probe_in(config: &Path, id: &str) -> Probe {
    if !valid_id(id) {
        return Probe::Unknown;
    }
    let Ok(projects) = fs::read_dir(config.join("projects")) else {
        return Probe::Unknown;
    };
    if projects
        .filter_map(std::result::Result::ok)
        .any(|entry| entry.path().join(format!("{id}.jsonl")).is_file())
    {
        Probe::Present
    } else {
        // Project directory names are lossy and storage is an internal detail.
        // Absence is never proof that Claude cannot resume the session.
        Probe::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Observation, Provider, SessionIdReader, StopPolicy};
    use super::*;
    use std::time::Duration;

    const ID: &str = "f93528aa-d8d0-4bc7-bd1e-bfe80a099a5c";

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rabi-claude-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        dir
    }
    fn init(id: &str, mode: &str) -> String {
        serde_json::json!({"type":"system","subtype":"init","session_id":id,"permissionMode":mode})
            .to_string()
    }
    fn result(answer: &str, error: bool, id: &str) -> String {
        serde_json::json!({"type":"result","session_id":id,"is_error":error,"result":answer})
            .to_string()
    }
    fn finish(dir: &Path, text: &str) -> Final {
        fs::write(dir.join("events.jsonl"), text).unwrap();
        finalize(&dir.join("events.jsonl")).unwrap()
    }

    #[test]
    fn new_and_resume_argv_and_saved_prompt_stdin_are_exact() {
        let dir = temp_dir();
        let workspace = dir.join("workspace 二");
        fs::create_dir(&workspace).unwrap();
        fs::write(dir.join("prompt.md"), b"saved prompt\nsecond line\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let binary = dir.join("claude");
        fs::write(&binary, "#!/bin/sh\n/bin/cat\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        for (session, flag) in [
            (
                Session::New {
                    requested: Some(ID),
                },
                "--session-id",
            ),
            (Session::Resume { id: ID }, "--resume"),
        ] {
            let mut command =
                command_with_binary(binary.clone(), &workspace, &dir, session, None).unwrap();
            assert_eq!(command.get_program(), binary.as_os_str());
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    flag,
                    ID,
                    "--permission-mode",
                    "auto",
                    "--permission-prompts",
                    "none",
                    "--model",
                    "opus",
                    "--effort",
                    "high"
                ]
            );
            assert_eq!(command.get_current_dir(), Some(workspace.as_path()));
            assert_eq!(command.get_envs().count(), 0);
            // The saved prompt is already open even if its pathname changes.
            fs::rename(dir.join("prompt.md"), dir.join("original.md")).unwrap();
            fs::write(dir.join("prompt.md"), b"replacement").unwrap();
            let output = command.output().unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"saved prompt\nsecond line\n");
            fs::rename(dir.join("original.md"), dir.join("prompt.md")).unwrap();
        }
        assert!(command_with_binary(
            PathBuf::from("/bin/cat"),
            &workspace,
            &dir,
            Session::New { requested: None },
            None,
        )
        .is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn observes_nonempty_ids_and_parses_every_init_permission_mode() {
        assert_eq!(session_id(&init(ID, "auto")).as_deref(), Some(ID));
        assert_eq!(session_id(&result("x", false, ID)).as_deref(), Some(ID));
        for line in [
            "broken",
            "{}",
            r#"{"session_id":""}"#,
            r#"{"session_id":5}"#,
        ] {
            assert!(session_id(line).is_none());
        }
        assert!(permission_mode(&init(ID, "auto")).unwrap().is_ok());
        for mode in ["default", "plan", "bypassPermissions", ""] {
            assert!(permission_mode(&init(ID, mode))
                .unwrap()
                .unwrap_err()
                .contains("not `auto`"));
        }
        for line in [
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"system","subtype":"init","permissionMode":null}"#,
            r#"{"type":"system","subtype":"init","permissionMode":7}"#,
        ] {
            assert!(permission_mode(line)
                .unwrap()
                .unwrap_err()
                .contains("missing or invalid"));
        }
        assert!(permission_mode(&result("x", false, ID)).is_none());
        assert!(permission_mode("broken").is_none());
    }

    #[test]
    fn observer_keeps_validating_after_first_id_and_reports_init_without_id() {
        let events = format!("{{\"session_id\":\"{ID}\"}}\n{{\"type\":\"system\",\"subtype\":\"init\",\"permissionMode\":\"auto\"}}\n{}\n", init(ID, "default"));
        let mut reader = SessionIdReader::new(Provider::Claude, std::io::Cursor::new(events));
        let Observation {
            session_id,
            validated,
            error,
        } = reader.poll_observation().unwrap().unwrap();
        assert_eq!(session_id.as_deref(), Some(ID));
        assert!(!validated);
        assert!(error.is_none());
        let event = reader.poll_observation().unwrap().unwrap();
        assert!(event.validated);
        assert!(event.session_id.is_none());
        let event = reader.poll_observation().unwrap().unwrap();
        assert!(!event.validated);
        assert!(event.error.unwrap().contains("permission mode `default`"));
    }

    #[test]
    fn observer_accepts_large_tool_lines_without_losing_later_permission_checks() {
        let events = format!(
            "{}\n{}\n{}\n",
            init(ID, "auto"),
            serde_json::json!({"type":"assistant","session_id":ID,"text":"x".repeat(128*1024)}),
            init(ID, "default")
        );
        let mut reader = SessionIdReader::new(Provider::Claude, std::io::Cursor::new(events));
        assert!(reader.poll_observation().unwrap().unwrap().validated);
        let rejected = loop {
            if let Some(event) = reader.poll_observation().unwrap() {
                break event;
            }
        };
        assert!(!rejected.validated);
        assert!(rejected
            .error
            .unwrap()
            .contains("permission mode `default`"));
    }

    #[test]
    fn large_init_metadata_is_validated_in_the_same_poll() {
        for mode in [Some("auto"), Some("default"), None] {
            let mut event = serde_json::json!({"type":"system","subtype":"init","session_id":ID,"metadata":"x".repeat(128*1024)});
            if let Some(mode) = mode {
                event["permissionMode"] = mode.into();
            }
            let mut reader =
                SessionIdReader::new(Provider::Claude, std::io::Cursor::new(format!("{event}\n")));
            let observed = reader.poll_observation().unwrap().unwrap();
            assert_eq!(observed.validated, mode == Some("auto"));
            assert_eq!(observed.error.is_some(), mode != Some("auto"));
        }
    }

    #[test]
    fn observer_errors_on_uninspectable_events_without_unbounded_allocation() {
        for bytes in [
            b"\xff\n".to_vec(),
            b"not-json\n".to_vec(),
            vec![b'x'; super::super::stream::FINAL_LINE_BYTES + 1],
        ] {
            let mut reader = SessionIdReader::new(Provider::Claude, std::io::Cursor::new(bytes));
            let mut failed = false;
            for _ in 0..3 {
                if reader.poll_observation().is_err() {
                    failed = true;
                    break;
                }
            }
            assert!(
                failed,
                "uninspectable events must not silently disable validation"
            );
        }
    }

    #[test]
    fn finalize_keeps_identity_validation_failed_across_later_auto_inits() {
        let dir = temp_dir();
        for events in [
            format!(
                "{{\"session_id\":\"{ID}\"}}\n{}\n{}",
                init("other", "auto"),
                result("wrong init", false, ID)
            ),
            format!(
                "{}\n{}\n{}\n{}",
                init(ID, "auto"),
                init("other", "auto"),
                init(ID, "auto"),
                result("restored init", false, ID)
            ),
            format!(
                "{}\n{}",
                init(ID, "auto"),
                result("wrong result", false, "other")
            ),
            format!(
                "{}\n{}\n{}",
                init(ID, "auto"),
                serde_json::json!({"type":"result","session_id":"other","is_error":true,"result":null}),
                result("later valid", false, ID)
            ),
            format!(
                "{}\n{}",
                init(ID, "auto"),
                serde_json::json!({"type":"result","session_id":"other","is_error":true,"result":null})
            ),
        ] {
            let finalized = finish(&dir, &events);
            assert!(!finalized.session_validated);
            assert!(finalized.protocol_error.unwrap().contains("session id"));
            assert!(!dir.join("answer.md").exists());
        }
        for events in [
            format!(
                "{}\n{}",
                init(ID, "default"),
                result("wrong result", false, "other")
            ),
            format!(
                "{}\n{}\n{}",
                init(ID, "default"),
                init(ID, "auto"),
                result("later auto", false, ID)
            ),
        ] {
            let finalized = finish(&dir, &events);
            assert!(!finalized.session_validated);
            assert!(finalized
                .protocol_error
                .unwrap()
                .contains("permission mode `default`"));
            assert!(!dir.join("answer.md").exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn probe_uses_isolated_home_and_config_environment() {
        const CHILD: &str = "RABI_CLAUDE_PROBE_TEST_CHILD";
        if let Some(expected) = std::env::var_os(CHILD) {
            let expected = if expected == "present" {
                Probe::Present
            } else {
                Probe::Unknown
            };
            assert_eq!(probe_session(ID).unwrap(), expected);
            assert_eq!(probe_session("../outside").unwrap(), Probe::Unknown);
            return;
        }
        let root = temp_dir();
        let home = root.join("home");
        let config = root.join("config");
        fs::create_dir_all(home.join(".claude/projects/home-project")).unwrap();
        fs::write(
            home.join(format!(".claude/projects/home-project/{ID}.jsonl")),
            "",
        )
        .unwrap();
        fs::create_dir_all(config.join("projects/config-project")).unwrap();
        for (setting, expected) in [
            (None, "present"),
            (Some(""), "present"),
            (Some(config.to_str().unwrap()), "unknown"),
        ] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "provider::claude::tests::probe_uses_isolated_home_and_config_environment",
                    "--nocapture",
                ])
                .env("HOME", &home)
                .env(CHILD, expected)
                .env_remove("CLAUDE_CONFIG_DIR");
            if let Some(setting) = setting {
                command.env("CLAUDE_CONFIG_DIR", setting);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        fs::write(
            config.join(format!("projects/config-project/{ID}.jsonl")),
            "",
        )
        .unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider::claude::tests::probe_uses_isolated_home_and_config_environment",
                "--nocapture",
            ])
            .env("HOME", &home)
            .env("CLAUDE_CONFIG_DIR", &config)
            .env(CHILD, "present")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finalize_saves_last_result_without_newline_and_accepts_large_lines() {
        let dir = temp_dir();
        let answer = format!("{}\n二", "x".repeat(128 * 1024));
        let finalized = finish(
            &dir,
            &format!(
                "{}\n{}\n{}",
                init(ID, "auto"),
                result("old", false, ID),
                result(&answer, true, ID)
            ),
        );
        assert_eq!(finalized.session_id, ID);
        assert!(finalized.session_validated);
        assert!(finalized.is_error);
        assert!(finalized.protocol_error.is_none());
        assert_eq!(fs::read(dir.join("answer.md")).unwrap(), answer.as_bytes());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finalize_rejects_missing_truncated_malformed_mismatched_and_oversized_results() {
        let dir = temp_dir();
        for events in [
            String::new(),
            init(ID, "auto"),
            result("no init", false, ID),
            format!(
                "{}\n{}\n{{\"type\":\"result\"",
                init(ID, "auto"),
                result("old", false, ID)
            ),
            format!("{}\n{{\"type\":\"result\",\"result\":5}}", init(ID, "auto")),
            format!(
                "{}\n{}",
                init(ID, "auto"),
                result("mismatch", false, "other")
            ),
            format!(
                "{{\"session_id\":\"other\"}}\n{}\n{}",
                init(ID, "auto"),
                result("mismatch", false, ID)
            ),
            format!(
                "{}\n{}",
                init(ID, "auto"),
                result(
                    &"x".repeat(super::super::stream::FINAL_LINE_BYTES),
                    false,
                    ID
                )
            ),
            format!("{}\n{}", init(ID, "default"), result("unsafe", false, ID)),
            format!(
                "{}\n{}\n{}",
                init(ID, "auto"),
                init(ID, "default"),
                result("unsafe", false, ID)
            ),
        ] {
            let finalized = finish(&dir, &events);
            assert!(finalized.protocol_error.is_some());
            assert!(!dir.join("answer.md").exists());
        }
        fs::create_dir(dir.join("answer.md")).unwrap();
        let finalized = finish(
            &dir,
            &format!("{}\n{}", init(ID, "auto"), result("cannot save", false, ID)),
        );
        assert!(finalized
            .protocol_error
            .unwrap()
            .contains("Failed to write Claude answer"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn null_or_absent_interrupted_result_leaves_answer_absent_without_panicking() {
        let dir = temp_dir();
        for result in [
            serde_json::json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":null,"session_id":ID}),
            serde_json::json!({"type":"result","subtype":"error_during_execution","is_error":true,"session_id":ID}),
        ] {
            let finalized = finish(&dir, &format!("{}\n{result}", init(ID, "auto")));
            assert!(finalized.session_validated);
            assert!(finalized.protocol_error.is_some());
            assert!(!dir.join("answer.md").exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn probe_checks_config_and_home_and_never_proves_absence() {
        let dir = temp_dir();
        let home = dir.join("home");
        let alternate = dir.join("config");
        for config in [home.join(".claude"), alternate.clone()] {
            fs::create_dir_all(config.join("projects/lossy-workspace")).unwrap();
            assert_eq!(probe_in(&config, ID), Probe::Unknown);
            fs::write(
                config.join(format!("projects/lossy-workspace/{ID}.jsonl")),
                "",
            )
            .unwrap();
            assert_eq!(probe_in(&config, ID), Probe::Present);
            assert_eq!(
                probe_in(&config, &uuid::Uuid::new_v4().to_string()),
                Probe::Unknown
            );
        }
        assert_eq!(
            config_dir(None, Some(home.clone().into_os_string())),
            Some(home.join(".claude"))
        );
        assert_eq!(
            config_dir(Some("".into()), Some(home.clone().into_os_string())),
            Some(home.join(".claude"))
        );
        assert_eq!(
            config_dir(
                Some(alternate.clone().into_os_string()),
                Some(home.into_os_string())
            ),
            Some(alternate)
        );
        assert_eq!(config_dir(None, None), None);
        for invalid in [
            "",
            ".",
            "..",
            "../outside",
            "/tmp/file",
            "a/b",
            "not-uuid",
            "{f93528aa-d8d0-4bc7-bd1e-bfe80a099a5c}",
            "f93528aad8d04bc7bd1ebfe80a099a5c",
        ] {
            assert!(!valid_id(invalid));
            assert_eq!(probe_in(&dir, invalid), Probe::Unknown);
        }
        assert!(valid_id(ID));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn name_serialization_and_stop_policy_are_closed_and_exact() {
        assert_eq!("claude".parse::<Provider>().unwrap(), Provider::Claude);
        assert_eq!(Provider::Claude.name(), "claude");
        assert_eq!(Provider::Claude.binary(), "claude");
        assert_eq!(Provider::Claude.display_name(), "Claude");
        assert_eq!(
            serde_json::to_string(&Provider::Claude).unwrap(),
            "\"claude\""
        );
        assert_eq!(
            serde_json::from_str::<Provider>("\"claude\"").unwrap(),
            Provider::Claude
        );
        assert_eq!(
            Provider::Claude.stop_policy(),
            StopPolicy {
                signal: 2,
                grace: Duration::from_secs(2),
                force_signal: 9,
                term_grace: Some(Duration::from_secs(1))
            }
        );
    }
}
