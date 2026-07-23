//! User configuration loaded from `oboro.toml`.
//!
//! The file is optional: without one, the deterministic recognisers run with
//! French defaults and no user patterns.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;

use crate::detect::EntityKind;

/// The file name looked up in the working directory and its ancestors.
pub const CONFIG_FILE: &str = "oboro.toml";

/// Region used when a file does not set one.
const DEFAULT_REGION: &str = "FR";
/// Whether the recognition model runs when installed, absent configuration.
const DEFAULT_NER_ENABLED: bool = true;
/// Minimum model probability acted on, absent configuration.
const DEFAULT_NER_THRESHOLD: f32 = 0.15;
/// Whether output filenames are redacted, absent configuration.
const DEFAULT_REDACT_FILENAMES: bool = true;

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
    /// Whether to run the local recognition model when it is installed.
    pub ner_enabled: bool,
    /// Minimum probability before a model detection is acted on.
    ///
    /// Calibrated against the quantised export, and against whole documents
    /// rather than single sentences: the same name scores 0.47 alone but
    /// 0.24 once surrounded by context, so a threshold tuned on sentences
    /// silently misses names in real files.
    ///
    /// Ordinary prose misread as a name tops out around 0.13, which leaves
    /// this default a narrow margin. It errs towards redacting, because a
    /// false positive costs one allowlist entry and a false negative is a
    /// name reaching a language model.
    pub ner_threshold: f32,
    /// Values that must never be redacted, such as the user's own company.
    pub allowlist: Vec<String>,
    /// The allowlist folded once for lookup, so [`Config::is_allowlisted`] does
    /// not trim and lowercase every entry on every candidate span.
    allowlist_folded: HashSet<String>,
    pub denylist: Vec<DenyTerm>,
    pub patterns: Vec<CustomPattern>,
    /// Whether PII detected in the input filename is redacted in the output name.
    pub redact_filenames: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_region: DEFAULT_REGION.to_owned(),
            ner_enabled: DEFAULT_NER_ENABLED,
            ner_threshold: DEFAULT_NER_THRESHOLD,
            allowlist: Vec::new(),
            allowlist_folded: HashSet::new(),
            denylist: Vec::new(),
            patterns: Vec::new(),
            redact_filenames: DEFAULT_REDACT_FILENAMES,
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

    /// Searches `start` and its ancestors for a `oboro.toml`.
    #[must_use]
    pub fn discover(start: &Path) -> Option<PathBuf> {
        start.ancestors().find_map(|dir| {
            let candidate = dir.join(CONFIG_FILE);
            candidate.is_file().then_some(candidate)
        })
    }

    /// Searches the working directory and its ancestors for a `oboro.toml`.
    #[must_use]
    pub fn discover_from_cwd() -> Option<PathBuf> {
        std::env::current_dir()
            .ok()
            .and_then(|dir| Self::discover(&dir))
    }

    /// Whether `text` matches an allowlist entry, ignoring case and
    /// surrounding whitespace.
    ///
    /// Case folding is Unicode-aware: ASCII-only folding would leave
    /// `SOCIÉTÉ` and `Société` unequal, silently failing for exactly the
    /// French text this tool targets.
    #[must_use]
    pub fn is_allowlisted(&self, text: &str) -> bool {
        self.allowlist_folded.contains(&fold(text))
    }
}

/// Folds a value for allowlist comparison: trimmed and lowercased, Unicode
/// aware so accented French text folds correctly.
fn fold(text: &str) -> String {
    text.trim().to_lowercase()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default = "default_region")]
    default_region: String,
    #[serde(default = "default_ner_enabled")]
    ner_enabled: bool,
    #[serde(default = "default_ner_threshold")]
    ner_threshold: f32,
    #[serde(default = "default_redact_filenames")]
    redact_filenames: bool,
    #[serde(default)]
    allowlist: Vec<String>,
    #[serde(default)]
    denylist: Vec<RawDenyTerm>,
    #[serde(default)]
    patterns: Vec<RawPattern>,
}

fn default_region() -> String {
    DEFAULT_REGION.to_owned()
}

fn default_ner_enabled() -> bool {
    DEFAULT_NER_ENABLED
}

fn default_ner_threshold() -> f32 {
    DEFAULT_NER_THRESHOLD
}

fn default_redact_filenames() -> bool {
    DEFAULT_REDACT_FILENAMES
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDenyTerm {
    term: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    case_sensitive: bool,
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
                    .case_insensitive(!entry.case_sensitive)
                    .build()
                    .with_context(|| format!("compiling denylist term '{term}'"))?;
                Ok(DenyTerm { kind, regex })
            })
            .collect::<Result<Vec<_>>>()?;

        if !(0.0..=1.0).contains(&self.ner_threshold) {
            bail!(
                "ner_threshold must be between 0.0 and 1.0, got {}",
                self.ner_threshold
            );
        }

        let allowlist_folded = self.allowlist.iter().map(|entry| fold(entry)).collect();

        Ok(Config {
            default_region: self.default_region,
            ner_enabled: self.ner_enabled,
            ner_threshold: self.ner_threshold,
            allowlist: self.allowlist,
            allowlist_folded,
            denylist,
            patterns,
            redact_filenames: self.redact_filenames,
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
    fn case_sensitive_denylist_terms_match_exact_case_only() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(
            dir.path(),
            r#"
[[denylist]]
term = "IT"
case_sensitive = true
"#,
        );
        let config = Config::load(Some(&path)).expect("configuration must load");
        let regex = &config.denylist[0].regex;
        assert!(regex.is_match("the IT department"));
        assert!(!regex.is_match("read it now"));
    }

    #[test]
    fn the_allowlist_folds_accented_case() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = write(dir.path(), "allowlist = [\"Société Générale\"]\n");
        let config = Config::load(Some(&path)).expect("configuration must load");
        assert!(
            config.is_allowlisted("SOCIÉTÉ GÉNÉRALE"),
            "ASCII-only folding would leave the accented letters unequal"
        );
        assert!(config.is_allowlisted("  société générale  "));
        assert!(!config.is_allowlisted("Société Anonyme"));
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
