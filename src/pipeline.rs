//! The two directions of the tool: sanitising text before it reaches a model,
//! and restoring the model's answer afterwards.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;

use crate::config::Config;
use crate::detect::{merge, rules::Rules};
use crate::vault::Vault;

/// Matches placeholders such as `[[PHONE_2]]` or `[[CONTRACT_NUMBER_1]]`.
///
/// The tag capture is greedy so that the trailing `_<number>` binds to the
/// sequence, leaving multi-word custom tags intact.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[([A-Z][A-Z0-9_]*)_(\d+)\]\]").expect("placeholder pattern is valid")
});

/// What `clean` did to a document.
pub struct CleanReport {
    pub text: String,
    /// Number of entities replaced, counted per occurrence.
    pub replaced: usize,
    /// Occurrences per placeholder tag, for a human-readable summary.
    pub by_tag: BTreeMap<String, usize>,
}

/// What `restore` did to a model's answer.
pub struct RestoreReport {
    pub text: String,
    pub restored: usize,
    /// Placeholders this vault has never issued; left untouched in the output.
    pub unknown: usize,
}

/// Replaces every detected entity in `text` with a stable placeholder.
///
/// # Errors
///
/// Returns an error if the vault cannot allocate a placeholder, for instance
/// when its database is unreadable.
pub fn clean(text: &str, config: &Config, vault: &mut Vault) -> Result<CleanReport> {
    let spans = merge::resolve(detect_all(text, config)?);

    let mut output = String::with_capacity(text.len());
    let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();
    let mut replaced = 0;
    let mut cursor = 0;

    // `resolve` returns disjoint spans in source order, so one forward pass
    // covers the text without ever revisiting what it has written.
    for span in &spans {
        let placeholder = vault
            .placeholder_for(&span.kind, &span.text)
            .with_context(|| format!("allocating a placeholder for a {} entity", span.kind))?;
        output.push_str(&text[cursor..span.start]);
        output.push_str(&placeholder);
        cursor = span.end;
        *by_tag.entry(span.kind.tag()).or_default() += 1;
        replaced += 1;
    }
    output.push_str(&text[cursor..]);

    Ok(CleanReport {
        text: output,
        replaced,
        by_tag,
    })
}

/// Runs every detection layer available in this build.
///
/// Layers are independent and may overlap; `merge::resolve` decides between
/// them. Adding a layer is adding to this list.
// Without the ner feature there is only one layer, so the accumulator needs
// no mutation and nothing here can fail. Both stay for the build that has it.
#[cfg_attr(not(feature = "ner"), allow(clippy::unnecessary_wraps))]
fn detect_all(text: &str, config: &Config) -> Result<Vec<crate::detect::Span>> {
    #[cfg_attr(not(feature = "ner"), allow(unused_mut))]
    let mut spans = Rules::new(config).detect(text);

    #[cfg(feature = "ner")]
    if let Some(recogniser) = crate::detect::ner::load_if_available(config)? {
        spans.extend(recogniser.detect(text)?);
    }

    Ok(spans)
}

/// Puts the real values back into a model's answer.
///
/// Placeholders the vault does not know are left in place: they are more
/// likely to be text the model invented than a mapping to recover, and
/// silently deleting them would corrupt the answer.
///
/// # Errors
///
/// Returns an error if the vault cannot be read or a stored value cannot be
/// decrypted.
pub fn restore(text: &str, vault: &Vault) -> Result<RestoreReport> {
    let mut output = String::with_capacity(text.len());
    let mut restored = 0;
    let mut unknown = 0;
    let mut cursor = 0;

    for capture in PLACEHOLDER.captures_iter(text) {
        let (Some(found), Some(tag), Some(seq)) = (capture.get(0), capture.get(1), capture.get(2))
        else {
            continue;
        };
        // A sequence too large to be one of ours cannot resolve. Skipping
        // without advancing the cursor leaves the text exactly as it was.
        let Ok(seq) = seq.as_str().parse::<i64>() else {
            unknown += 1;
            continue;
        };

        output.push_str(&text[cursor..found.start()]);
        if let Some(value) = vault.value_for(tag.as_str(), seq)? {
            output.push_str(&value);
            restored += 1;
        } else {
            output.push_str(found.as_str());
            unknown += 1;
        }
        cursor = found.end();
    }
    output.push_str(&text[cursor..]);

    Ok(RestoreReport {
        text: output,
        restored,
        unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;

    struct Fixture {
        vault: Vault,
        config: Config,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temporary directory");
            let vault = Vault::open(&dir.path().join("vault.db"), &dir.path().join("key"))
                .expect("opening a vault");
            Self {
                vault,
                config: Config::default(),
                _dir: dir,
            }
        }
    }

    #[test]
    fn cleaning_removes_the_original_values() {
        let mut fixture = Fixture::new();
        let text = "Contact jean.dupont@example.com on 06 12 34 56 78.";
        let report = clean(text, &fixture.config, &mut fixture.vault).expect("cleaning");
        assert!(!report.text.contains("jean.dupont@example.com"));
        assert!(!report.text.contains("06 12 34 56 78"));
        assert_eq!(report.replaced, 2);
    }

    #[test]
    fn cleaning_and_restoring_reproduces_the_original() {
        let mut fixture = Fixture::new();
        let text = "Jean can be reached at jean@example.com or 06 12 34 56 78.";
        let cleaned = clean(text, &fixture.config, &mut fixture.vault).expect("cleaning");
        let restored = restore(&cleaned.text, &fixture.vault).expect("restoring");
        assert_eq!(restored.text, text);
        assert_eq!(restored.unknown, 0);
    }

    #[test]
    fn repeated_values_share_one_placeholder() {
        let mut fixture = Fixture::new();
        let text = "Write to a@example.com; a@example.com replies fast.";
        let report = clean(text, &fixture.config, &mut fixture.vault).expect("cleaning");
        assert_eq!(report.replaced, 2);
        assert_eq!(report.text.matches("[[EMAIL_1]]").count(), 2);
    }

    /// The rules layer alone must not touch text with nothing in it. The
    /// model layer is probabilistic and may over-redact prose, which is
    /// covered by its own calibration tests.
    #[test]
    fn text_without_entities_is_unchanged_by_the_rules_layer() {
        let mut fixture = Fixture::new();
        fixture.config.ner_enabled = false;
        let text = "The quick brown fox jumps over the lazy dog.";
        let report = clean(text, &fixture.config, &mut fixture.vault).expect("cleaning");
        assert_eq!(report.text, text);
        assert_eq!(report.replaced, 0);
    }

    #[test]
    fn empty_input_stays_empty() {
        let mut fixture = Fixture::new();
        let report = clean("", &fixture.config, &mut fixture.vault).expect("cleaning");
        assert_eq!(report.text, "");
    }

    #[test]
    fn unknown_placeholders_survive_restoration() {
        let fixture = Fixture::new();
        let report = restore("See [[PERSON_9]] for details.", &fixture.vault).expect("restoring");
        assert_eq!(report.text, "See [[PERSON_9]] for details.");
        assert_eq!(report.unknown, 1);
        assert_eq!(report.restored, 0);
    }

    #[test]
    fn restoring_text_without_placeholders_changes_nothing() {
        let fixture = Fixture::new();
        let report = restore("Nothing to see here.", &fixture.vault).expect("restoring");
        assert_eq!(report.text, "Nothing to see here.");
        assert_eq!(report.restored, 0);
    }

    #[test]
    fn multi_word_custom_tags_round_trip() {
        let mut fixture = Fixture::new();
        let placeholder = fixture
            .vault
            .placeholder_for(
                &crate::detect::EntityKind::Custom("contract number".to_owned()),
                "CT-123456",
            )
            .expect("allocating");
        assert_eq!(placeholder, "[[CONTRACT_NUMBER_1]]");
        let report = restore(&format!("Ref {placeholder}."), &fixture.vault).expect("restoring");
        assert_eq!(report.text, "Ref CT-123456.");
    }

    #[test]
    fn restoration_preserves_surrounding_text_exactly() {
        let mut fixture = Fixture::new();
        let cleaned = clean(
            "Line one\n\nMail: a@example.com\n",
            &fixture.config,
            &mut fixture.vault,
        )
        .expect("cleaning");
        let restored = restore(&cleaned.text, &fixture.vault).expect("restoring");
        assert_eq!(restored.text, "Line one\n\nMail: a@example.com\n");
    }
}
