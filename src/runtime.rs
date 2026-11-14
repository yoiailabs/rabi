//! Invocation-scoped execution settings; never mutate the process environment.
use std::cell::Cell;
use std::io::IsTerminal;

use crate::error::Error;

thread_local! {
    static NO_INPUT: Cell<bool> = const { Cell::new(false) };
    static HOOK: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn non_interactive() -> bool {
    NO_INPUT.get()
        || crate::output::json()
        || std::env::var_os("CI").is_some_and(|value| !value.is_empty())
        || std::env::var_os("RABI_NO_INPUT").is_some_and(|value| !value.is_empty() && value != "0")
        || !std::io::stdin().is_terminal()
        || !std::io::stderr().is_terminal()
}

pub(crate) fn is_hook() -> bool {
    HOOK.get()
}
pub(crate) struct HookScope(bool);
pub(crate) fn hook_scope() -> HookScope {
    HookScope(HOOK.replace(true))
}
impl Drop for HookScope {
    fn drop(&mut self) {
        HOOK.set(self.0);
    }
}

/// One common-option scan resolves output and help before command validation.
/// Value/remainder arity comes from the same definitions as the command parser.
pub(crate) struct Prepared {
    pub args: Vec<String>,
    pub json: bool,
    pub help: bool,
    pub error: Option<Error>,
}

pub(crate) fn prepare(
    args: &[String],
    supports_json: bool,
    key: &str,
    options: &[crate::args::Opt],
    help_available: bool,
) -> Prepared {
    NO_INPUT.set(false);
    let mut remaining = Vec::new();
    let mut explicit = None;
    let mut text_listing = false;
    let mut help = false;
    let mut error = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            remaining.extend_from_slice(&args[index..]);
            break;
        }
        let (name, inline) = arg
            .split_once('=')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((arg.as_str(), None));
        match name {
            "--no-input" if inline.is_none() => NO_INPUT.set(true),
            "--json" if inline.is_none() => explicit = Some(true),
            "--output" => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        index += 1;
                        let Some(value) = args.get(index).map(String::as_str) else {
                            error.get_or_insert_with(|| {
                                Error::validation(format!("Option '{name}' argument missing"))
                            });
                            break;
                        };
                        value
                    }
                };
                match value {
                    "json" => explicit = Some(true),
                    "text" => explicit = Some(false),
                    _ => {
                        error.get_or_insert_with(|| {
                            Error::validation("Invalid output mode; use text or json")
                        });
                    }
                }
            }
            "--help" | "-h" if inline.is_none() && help_available => help = true,
            _ => {
                if inline.is_none() && matches!((key, name), ("list", "--names" | "--paths")) {
                    text_listing = true;
                }
                remaining.push(arg.clone());
                let spec = options.iter().find_map(|spec| {
                    if let Some(long) = name.strip_prefix("--") {
                        (long == spec.name).then_some((spec, inline.is_some()))
                    } else {
                        let short = arg.strip_prefix('-')?;
                        short
                            .chars()
                            .next()
                            .is_some_and(|ch| spec.short == Some(ch))
                            .then_some((spec, short.len() > 1))
                    }
                });
                if let Some((spec, attached)) = spec {
                    if spec.value && !attached {
                        if spec.remainder {
                            let values = &args[index + 1..];
                            if values.is_empty() || (values.len() == 1 && values[0] == "--") {
                                error.get_or_insert_with(|| {
                                    Error::validation(format!(
                                        "Option '{name} <value>' argument missing"
                                    ))
                                });
                            }
                            remaining.extend_from_slice(values);
                            break;
                        }
                        if let Some(value) = args.get(index + 1) {
                            remaining.push(value.clone());
                            index += 1;
                        } else {
                            error.get_or_insert_with(|| {
                                Error::validation(format!(
                                    "Option '{name} <value>' argument missing"
                                ))
                            });
                        }
                    }
                } else if key == "exec" && !arg.starts_with('-') {
                    // After the worktree name, even help/output flags are child argv.
                    remaining.extend_from_slice(&args[index + 1..]);
                    break;
                }
            }
        }
        index += 1;
    }
    // Help ignores output defaults. Existing exec help also ignores unsupported
    // JSON requests; script commands always refuse explicit JSON output.
    // Invalid or missing option values take precedence over a help request.
    if !supports_json
        && explicit == Some(true)
        && (!help || matches!(key, "completions" | "shell-init"))
    {
        error.get_or_insert_with(|| Error::validation("This command does not support JSON output"));
    }
    let env_json = if supports_json && (!help || error.is_some()) {
        match std::env::var("RABI_OUTPUT").ok().as_deref() {
            None | Some("") | Some("text") => false,
            Some("json") => true,
            _ => {
                error.get_or_insert_with(|| {
                    Error::validation("Invalid RABI_OUTPUT; use text or json")
                });
                false
            }
        }
    } else {
        false
    };
    let json = supports_json
        && (!help || error.is_some())
        && explicit.unwrap_or(env_json && !text_listing);
    Prepared {
        args: remaining,
        json,
        help,
        error,
    }
}
