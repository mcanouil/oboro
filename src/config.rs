//! User configuration loaded from `hush.toml`.
//!
//! The file is optional: without one, the deterministic recognisers run with
//! French defaults and no user patterns.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::detect::EntityKind;

/// The file name looked up in the working directory and its ancestors.
pub const CONFIG_FILE: &str = "hush.toml";

/// A user-defined pattern, such as a contract number format.
pub struct CustomPattern {
    pub name: String,
    pub regex: Regex,
}

/// A literal term that must always be redacted.
pub struct DenyTerm {
    pub kind: EntityKind,
    pub regex: Regex,
}

/// Effective configuration for a run.
pub struct Config {
    /// Region used to interpret national phone number formats.
    pub default_region: String,
    /// Values that must never be redacted, such as the user's own company.
    pub allowlist: Vec<String>,
    pub denylist: Vec<DenyTerm>,
    pub patterns: Vec<CustomPattern>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_region: "FR".to_owned(),
            allowlist: Vec::new(),
            denylist: Vec::new(),
            patterns: Vec::new(),
        }
    }
}

impl Config {
    /// Loads configuration from `path`, or returns defaults when `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, is not valid TOML, or
    /// contains a pattern that is not a valid regular expression.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading configuration from {}", path.display()))?;
        let parsed: RawConfig = toml::from_str(&raw)
            .with_context(|| format!("parsing configuration from {}", path.display()))?;
        parsed
            .compile()
            .with_context(|| format!("applying configuration from {}", path.display()))
    }

    /// Searches `start` and its ancestors for a `hush.toml`.
    #[must_use]
    pub fn discover(start: &Path) -> Option<PathBuf> {
        start.ancestors().find_map(|dir| {
            let candidate = dir.join(CONFIG_FILE);
            candidate.is_file().then_some(candidate)
        })
    }

    /// Whether `text` matches an allowlist entry, ignoring case and
    /// surrounding whitespace.
    #[must_use]
    pub fn is_allowlisted(&self, text: &str) -> bool {
        let needle = text.trim();
        self.allowlist
            .iter()
            .any(|entry| entry.trim().eq_ignore_ascii_case(needle))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default = "default_region")]
    default_region: String,
    #[serde(default)]
    allowlist: Vec<String>,
    #[serde(default)]
    denylist: Vec<RawDenyTerm>,
    #[serde(default)]
    patterns: Vec<RawPattern>,
}

fn default_region() -> String {
    "FR".to_owned()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDenyTerm {
    term: String,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPattern {
    name: String,
    regex: String,
}

impl RawConfig {
    fn compile(self) -> Result<Config> {
        let patterns = self
            .patterns
            .into_iter()
            .map(|pattern| {
                let regex = Regex::new(&pattern.regex).with_context(|| {
                    format!(
                        "compiling pattern '{}'; check the regular expression syntax",
                        pattern.name
                    )
                })?;
                Ok(CustomPattern {
                    name: pattern.name,
                    regex,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let denylist = self
            .denylist
            .into_iter()
            .map(|entry| {
                let term = entry.term.trim();
                if term.is_empty() {
                    bail!("a denylist term is empty; an empty term would match everywhere");
                }
                let kind = entry
                    .kind
                    .as_deref()
                    .map_or(EntityKind::Organisation, parse_kind);
                let regex = RegexBuilder::new(&word_bounded(term))
                    .case_insensitive(true)
                    .build()
                    .with_context(|| format!("compiling denylist term '{term}'"))?;
                Ok(DenyTerm { kind, regex })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Config {
            default_region: self.default_region,
            allowlist: self.allowlist,
            denylist,
            patterns,
        })
    }
}

/// Anchors a literal term at word boundaries.
///
/// A boundary is only added on a side where the term itself ends in a word
/// character: `\b` next to punctuation would demand a word character there
/// and stop the term matching at all.
fn word_bounded(term: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let leading = if term.starts_with(is_word) { r"\b" } else { "" };
    let trailing = if term.ends_with(is_word) { r"\b" } else { "" };
    format!("{leading}{}{trailing}", regex::escape(term))
}

/// Maps a configuration string onto an entity kind, falling back to a custom
/// kind so unknown names still produce meaningful placeholders.
fn parse_kind(value: &str) -> EntityKind {
    match value.to_ascii_lowercase().as_str() {
        "person" | "client" | "name" => EntityKind::Person,
        "organisation" | "organization" | "org" | "provider" | "company" => {
            EntityKind::Organisation
        }
        "address" => EntityKind::Address,
        "phone" => EntityKind::Phone,
        "email" => EntityKind::Email,
        other => EntityKind::Custom(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join(CONFIG_FILE);
        std::fs::write(&path, body).expect("writing test configuration");
        path
    }

    #[test]
    fn missing_configuration_yields_defaults() {
        let config = Config::load(None).expect("defaults must load");
        assert_eq!(config.default_region, "FR");
        assert!(config.allowlist.is_empty());
    }

    #[test]
    fn parses_all_sections() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(
            dir.path(),
            r#"
default_region = "GB"
allowlist = ["Ma Societe SARL"]

[[denylist]]
term = "Acme Corp"
kind = "provider"

[[patterns]]
name = "contract number"
regex = "CT-[0-9]{6}"
"#,
        );

        let config = Config::load(Some(&path)).expect("configuration must load");
        assert_eq!(config.default_region, "GB");
        assert!(config.is_allowlisted("ma societe sarl"));
        assert_eq!(config.denylist.len(), 1);
        assert_eq!(config.denylist[0].kind, EntityKind::Organisation);
        assert_eq!(config.patterns[0].name, "contract number");
        assert!(config.patterns[0].regex.is_match("CT-123456"));
    }

    #[test]
    fn denylist_matches_ignore_case_but_respect_word_boundaries() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(
            dir.path(),
            r#"
[[denylist]]
term = "Acme"
"#,
        );
        let config = Config::load(Some(&path)).expect("configuration must load");
        let regex = &config.denylist[0].regex;
        assert!(regex.is_match("invoice from ACME today"));
        assert!(!regex.is_match("acmentioned"));
    }

    #[test]
    fn denylist_terms_ending_in_punctuation_still_match() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(
            dir.path(),
            r#"
[[denylist]]
term = "Acme S.A."
"#,
        );
        let config = Config::load(Some(&path)).expect("configuration must load");
        assert!(
            config.denylist[0]
                .regex
                .is_match("invoice from Acme S.A. today")
        );
    }

    #[test]
    fn empty_denylist_terms_are_rejected() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(
            dir.path(),
            r#"
[[denylist]]
term = "   "
"#,
        );
        let error = Config::load(Some(&path))
            .err()
            .expect("an empty term must fail");
        assert!(format!("{error:#}").contains("empty"));
    }

    #[test]
    fn invalid_regex_reports_the_offending_pattern() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(
            dir.path(),
            r#"
[[patterns]]
name = "broken"
regex = "CT-[0-9"
"#,
        );
        let error = Config::load(Some(&path))
            .err()
            .expect("invalid regex must fail");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("broken"), "unhelpful error: {rendered}");
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_silently_ignored() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(dir.path(), "defualt_region = \"FR\"\n");
        assert!(Config::load(Some(&path)).is_err());
    }

    #[test]
    fn discover_walks_up_to_an_ancestor() {
        let dir = tempfile::tempdir().expect("temporary directory");
        write(dir.path(), "default_region = \"FR\"\n");
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).expect("creating nested directories");
        assert!(Config::discover(&nested).is_some());
    }
}
