//! Minimal skill discovery, independent of run invocation.
use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(serde::Serialize)]
pub(crate) struct Skill {
    pub name: String,
    pub source: &'static str,
    pub path: PathBuf,
}

pub(crate) fn list(checkout: &Path) -> Result<Vec<Skill>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::msg("HOME is not set"))?;
    let locations = [
        (checkout.join(".agents/skills"), "checkout:.agents"),
        (checkout.join(".codex/skills"), "checkout:.codex"),
        (checkout.join(".claude/skills"), "checkout:.claude"),
        (home.join(".agents/skills"), "user:.agents"),
        (crate::provider::codex_home()?.join("skills"), "user:.codex"),
        (home.join(".claude/skills"), "user:.claude"),
    ];
    let mut skills = Vec::new();
    for (directory, source) in locations {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::msg(e.to_string())),
        };
        let mut found = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| Error::msg(e.to_string()))?;
            let path = entry.path();
            if path.is_dir() && path.join("SKILL.md").is_file() {
                found.push(Skill {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    source,
                    path,
                });
            }
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        skills.extend(found);
    }
    Ok(skills)
}
