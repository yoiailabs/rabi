//! Committed risk-policy schema and root-anchored glob matching.
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tier {
    Low,
    Medium,
    #[default]
    High,
}

impl fmt::Display for Tier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Policy {
    schema: u32,
    #[serde(default)]
    pub default_tier: Tier,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub kinds: Kinds,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            schema: 1,
            default_tier: Tier::High,
            rules: Vec::new(),
            limits: Limits::default(),
            kinds: Kinds::default(),
        }
    }
}

impl Policy {
    pub fn parse(text: &str) -> Result<Self> {
        let policy: Self = toml_edit::de::from_str(text)
            .map_err(|error| Error::msg(format!("Invalid risk policy: {error}")))?;
        if policy.schema != 1 {
            return Err(Error::msg("Invalid risk policy schema: expected 1"));
        }
        for (index, rule) in policy.rules.iter().enumerate() {
            if rule.paths.is_empty() {
                return Err(Error::msg(format!(
                    "Invalid risk policy rule[{index}].paths: must not be empty"
                )));
            }
            for path in &rule.paths {
                if path.is_empty() || path.contains('\0') {
                    return Err(Error::msg(format!("Invalid risk policy rule[{index}].paths: patterns must be nonempty and NUL-free")));
                }
            }
            if rule.tier.is_none() && !rule.floor {
                return Err(Error::msg(format!(
                    "Invalid risk policy rule[{index}].tier: required unless floor = true"
                )));
            }
        }
        for (key, limit) in [
            ("max-changed-lines", policy.limits.max_changed_lines),
            ("max-files", policy.limits.max_files),
        ] {
            if limit == Some(0) {
                return Err(Error::msg(format!(
                    "Invalid risk policy limits.{key}: must be positive"
                )));
            }
        }
        Ok(policy)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rule {
    pub paths: Vec<String>,
    pub tier: Option<Tier>,
    #[serde(default)]
    pub floor: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Limits {
    pub max_changed_lines: Option<u64>,
    pub max_files: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Kinds {
    pub binary: Tier,
    pub symlink: Tier,
    pub submodule: Tier,
    pub exec_bit: Tier,
}

/// Full, case-sensitive match against a repository-relative UTF-8 path.
/// `*` and `?` never cross `/`; `**` can cross `/`, and `**/` can also
/// consume zero directories. All other characters are literal, including dots,
/// brackets, backslashes, and `!`. No implicit basename or prefix matching.
pub(super) fn glob_matches(pattern: &str, path: &str) -> bool {
    #[derive(Clone, Copy)]
    enum Token {
        Literal(char),
        Any,
        Star,
        Globstar,
        Directories,
    }

    let mut tokens = Vec::new();
    let mut characters = pattern.chars().peekable();
    while let Some(character) = characters.next() {
        let token = match character {
            '*' => {
                let mut recursive = false;
                while characters.peek() == Some(&'*') {
                    characters.next();
                    recursive = true;
                }
                if recursive && characters.peek() == Some(&'/') {
                    characters.next();
                    Token::Directories
                } else if recursive {
                    Token::Globstar
                } else {
                    Token::Star
                }
            }
            '?' => Token::Any,
            literal => Token::Literal(literal),
        };
        tokens.push(token);
    }
    let path: Vec<char> = path.chars().collect();
    let mut next = vec![false; path.len() + 1];
    next[path.len()] = true;
    for token in tokens.into_iter().rev() {
        let mut current = vec![false; path.len() + 1];
        let mut directory_match = false;
        for offset in (0..=path.len()).rev() {
            // A **/ token consumes complete directories, never a partial name.
            if offset < path.len() && path[offset] == '/' {
                directory_match |= current[offset + 1];
            }
            current[offset] = match token {
                Token::Directories => next[offset] || directory_match,
                Token::Globstar => next[offset] || (offset < path.len() && current[offset + 1]),
                Token::Star => {
                    next[offset]
                        || (offset < path.len() && path[offset] != '/' && current[offset + 1])
                }
                Token::Any => offset < path.len() && path[offset] != '/' && next[offset + 1],
                Token::Literal(literal) => {
                    offset < path.len() && path[offset] == literal && next[offset + 1]
                }
            };
        }
        next = current;
    }
    next[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_table() {
        for (pattern, path, expected) in [
            ("docs/*", "docs/a", true),
            ("docs/*", "docs/a/b", false),
            ("docs/**", "docs/a/b", true),
            ("docs/**", "docsx/a", false),
            ("docs/**", "docs", false),
            ("docs/**", "nested/docs/a", false),
            ("**/a", "a", true),
            ("**/a", "fooa", false),
            ("***/a", "fooa", false),
            ("docs/**/a", "docs/fooa", false),
            ("a/**/b", "a/xb", false),
            ("**/a", "one/two/a", true),
            ("docs/**/a", "docs/a", true),
            ("docs/**/a", "docs/x/y/a", true),
            ("*", ".hidden", true),
            (".github/**", ".github/workflows/check.yml", true),
            ("docs/?", "docs/é", true),
            ("docs/?", "docs/ab", false),
            ("docs/?", "docs//", false),
            ("readme", "nested/readme", false),
            ("/readme", "readme", false),
            ("a[bc]", "ab", false),
            ("a[bc]", "a[bc]", true),
        ] {
            assert_eq!(
                glob_matches(pattern, path),
                expected,
                "{pattern:?} {path:?}"
            );
        }
    }

    #[test]
    fn invalid_policy_table() {
        for (text, key) in [
            ("schema = 2", "schema"),
            ("default-tier = 'low'", "schema"),
            ("schema = 1\nunknown = true", "unknown"),
            ("schema = 1\n[ci]", "ci"),
            ("schema = 1\ndefault-tier = 'safe'", "default-tier"),
            ("schema = 1\n[[rule]]\npaths = []\ntier = 'low'", "paths"),
            ("schema = 1\n[[rule]]\npaths = ['docs/**']", "tier"),
            (
                "schema = 1\n[[rule]]\npaths = ['docs/**']\nfloor = false",
                "tier",
            ),
            ("schema = 1\n[[rule]]\npaths = ['']\ntier = 'low'", "paths"),
            (
                "schema = 1\n[[rule]]\npaths = 'docs/**'\ntier = 'low'",
                "paths",
            ),
            ("schema = 1\n[[rule]]\npaths = ['a']\ntier = 'bad'", "tier"),
            (
                "schema = 1\n[[rule]]\npaths = ['a']\nfloor = true\nextra = 1",
                "extra",
            ),
            ("schema = 1\n[limits]\nmax-files = 0", "max-files"),
            (
                "schema = 1\n[limits]\nmax-changed-lines = -1",
                "max-changed-lines",
            ),
            ("schema = 1\n[limits]\nmax-files = '2'", "max-files"),
            ("schema = 1\n[limits]\nextra = 1", "extra"),
            ("schema = 1\n[kinds]\nbinary = 'bad'", "binary"),
            ("schema = 1\n[kinds]\nextra = 'low'", "extra"),
        ] {
            let error = Policy::parse(text).expect_err(text).to_string();
            assert!(error.contains(key), "{key}: {error}");
        }
    }
}
