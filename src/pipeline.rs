//! The two directions of the tool: sanitising text before it reaches a model,
//! and restoring the model's answer afterwards.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;

use crate::detect::{Detector, merge};
use crate::vault::Vault;

/// Matches placeholders such as `[[PHONE_2]]` or `[[CONTRACT_NUMBER_1]]`.
///
/// The tag capture is greedy so that the trailing `_<number>` binds to the
/// sequence, leaving multi-word custom tags intact. The first character may be
/// a digit: a custom pattern named `2fa code` sanitises to the tag `2FA_CODE`,
/// and requiring a letter here would leave `[[2FA_CODE_1]]` unrestorable.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[([A-Z0-9][A-Z0-9_]*)_(\d+)\]\]").expect("placeholder pattern is valid")
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
/// Returns an error if a detection layer fails or the vault cannot allocate a
/// placeholder, for instance when its database is unreadable.
pub fn clean(text: &str, detector: &Detector, vault: &mut Vault) -> Result<CleanReport> {
    let spans = detect(text, detector)?;
    apply(text, &spans, vault)
}

/// Redacts PII in a filename stem, rendering placeholders filesystem-safe:
/// `[[PERSON_1]]` becomes `PERSON_1`.
///
/// Sharing `vault` with [`clean`] means a value appearing in both the name and
/// the document body maps to the same placeholder. Unlike [`clean`], this is
/// deliberately one-way: [`restore`] only rewrites content, and the bare
/// `PERSON_1` form no longer matches [`struct@PLACEHOLDER`], so a filename is
/// never reverse-substituted. The real value stays recoverable from the vault
/// by its tag and sequence.
///
/// # Errors
///
/// Returns an error if a detection layer fails or the vault cannot allocate a
/// placeholder.
pub fn clean_stem(stem: &str, detector: &Detector, vault: &mut Vault) -> Result<String> {
    let spans = detect(stem, detector)?;
    let report = apply(stem, &spans, vault)?;
    Ok(PLACEHOLDER
        .replace_all(&report.text, "${1}_${2}")
        .into_owned())
}

/// Finds every entity in `text`, reconciled into a disjoint, ordered set.
///
/// Separate from [`apply`] so `review` can put the result in front of the
/// user before anything is written or stored in the vault.
///
/// # Errors
///
/// Returns an error if a detection layer fails.
pub fn detect(text: &str, detector: &Detector) -> Result<Vec<crate::detect::Span>> {
    Ok(merge::resolve(detector.detect(text)?))
}

/// Replaces `spans` in `text` with placeholders from the vault.
///
/// The spans must be disjoint and in source order, as [`detect`] returns
/// them. Passing a subset is how `review` honours what the user rejected.
///
/// # Errors
///
/// Returns an error if the vault cannot allocate a placeholder.
pub fn apply(text: &str, spans: &[crate::detect::Span], vault: &mut Vault) -> Result<CleanReport> {
    let mut output = String::with_capacity(text.len());
    let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();
    // A value repeated through the document maps to one placeholder, so its
    // vault lookup is worth doing once rather than per occurrence.
    let mut memo: HashMap<(String, String), String> = HashMap::new();
    let mut replaced = 0;
    let mut cursor = 0;

    // Disjoint spans in source order mean one forward pass covers the text
    // without ever revisiting what it has written.
    for span in spans {
        let tag = span.kind.tag();
        let key = (tag.clone(), span.kind.normalise(&span.text));
        let placeholder = if let Some(existing) = memo.get(&key) {
            existing.clone()
        } else {
            let allocated = vault
                .placeholder_for(&span.kind, &span.text)
                .with_context(|| format!("allocating a placeholder for a {} entity", span.kind))?;
            memo.insert(key, allocated.clone());
            allocated
        };
        output.push_str(&text[cursor..span.start]);
        output.push_str(&placeholder);
        cursor = span.end;
        *by_tag.entry(tag).or_default() += 1;
        replaced += 1;
    }
    output.push_str(&text[cursor..]);

    Ok(CleanReport {
        text: output,
        replaced,
        by_tag,
    })
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
    // A placeholder repeated through the answer resolves to one value, so its
    // lookup and decryption are worth doing once.
    let mut memo: HashMap<(String, i64), Option<String>> = HashMap::new();

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

        let key = (tag.as_str().to_owned(), seq);
        let value = if let Some(cached) = memo.get(&key) {
            cached.clone()
        } else {
            let looked_up = vault.value_for(tag.as_str(), seq)?;
            memo.insert(key, looked_up.clone());
            looked_up
        };

        output.push_str(&text[cursor..found.start()]);
        if let Some(value) = value {
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
    use crate::config::Config;
    use crate::detect::Detector;
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
            // These tests assert the deterministic rules layer; the model is
            // exercised by its own calibration tests, and leaving it on would
            // make the outcome depend on whether the machine has it installed.
            let mut config = Config::default();
            config.ner_enabled = false;
            Self {
                vault,
                config,
                _dir: dir,
            }
        }
    }

    #[test]
    fn cleaning_removes_the_original_values() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let text = "Contact jean.dupont@example.com on 06 12 34 56 78.";
        let report = clean(text, &detector, &mut fixture.vault).expect("cleaning");
        assert!(!report.text.contains("jean.dupont@example.com"));
        assert!(!report.text.contains("06 12 34 56 78"));
        assert_eq!(report.replaced, 2);
    }

    #[test]
    fn cleaning_and_restoring_reproduces_the_original() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let text = "Jean can be reached at jean@example.com or 06 12 34 56 78.";
        let cleaned = clean(text, &detector, &mut fixture.vault).expect("cleaning");
        let restored = restore(&cleaned.text, &fixture.vault).expect("restoring");
        assert_eq!(restored.text, text);
        assert_eq!(restored.unknown, 0);
    }

    #[test]
    fn repeated_values_share_one_placeholder() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let text = "Write to a@example.com; a@example.com replies fast.";
        let report = clean(text, &detector, &mut fixture.vault).expect("cleaning");
        assert_eq!(report.replaced, 2);
        assert_eq!(report.text.matches("[[EMAIL_1]]").count(), 2);
    }

    #[test]
    fn clean_stem_unwraps_placeholders_for_the_filesystem() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let stem = clean_stem("invoice for a@example.com", &detector, &mut fixture.vault)
            .expect("cleaning the stem");
        assert_eq!(stem, "invoice for EMAIL_1");
        assert!(!stem.contains('@'));
        assert!(!stem.contains('['));
        assert!(!stem.contains(']'));
    }

    #[test]
    fn clean_stem_leaves_a_clean_name_untouched() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let stem = clean_stem("quarterly-report", &detector, &mut fixture.vault)
            .expect("cleaning the stem");
        assert_eq!(stem, "quarterly-report");
    }

    #[test]
    fn a_filename_and_the_body_share_one_placeholder() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let report = clean("mail a@example.com", &detector, &mut fixture.vault).expect("cleaning");
        assert!(report.text.contains("[[EMAIL_1]]"));
        let stem =
            clean_stem("a@example.com", &detector, &mut fixture.vault).expect("cleaning the stem");
        assert_eq!(stem, "EMAIL_1");
    }

    /// The rules layer alone must not touch text with nothing in it. The
    /// model layer is probabilistic and may over-redact prose, which is
    /// covered by its own calibration tests.
    #[test]
    fn text_without_entities_is_unchanged_by_the_rules_layer() {
        let mut fixture = Fixture::new();
        fixture.config.ner_enabled = false;
        let detector = Detector::new(&fixture.config).expect("detector");
        let text = "The quick brown fox jumps over the lazy dog.";
        let report = clean(text, &detector, &mut fixture.vault).expect("cleaning");
        assert_eq!(report.text, text);
        assert_eq!(report.replaced, 0);
    }

    #[test]
    fn empty_input_stays_empty() {
        let mut fixture = Fixture::new();
        let detector = Detector::new(&fixture.config).expect("detector");
        let report = clean("", &detector, &mut fixture.vault).expect("cleaning");
        assert_eq!(report.text, "");
    }

    /// A custom pattern whose name sanitises to a digit-leading tag must still
    /// round-trip: the placeholder regex has to accept a leading digit, or the
    /// value is neither restored nor reported as unknown.
    #[test]
    fn digit_leading_custom_tags_round_trip() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let config_path = dir.path().join("oboro.toml");
        std::fs::write(
            &config_path,
            "ner_enabled = false\n[[patterns]]\nname = \"2fa code\"\nregex = \"CODE-[0-9]{4}\"\n",
        )
        .expect("writing configuration");
        let config = Config::load(Some(&config_path)).expect("loading configuration");
        let detector = Detector::new(&config).expect("detector");
        let mut vault = Vault::open(&dir.path().join("vault.db"), &dir.path().join("key"))
            .expect("opening a vault");

        let text = "Your CODE-1234 is valid.";
        let cleaned = clean(text, &detector, &mut vault).expect("cleaning");
        assert!(
            cleaned.text.contains("[[2FA_CODE_1]]"),
            "unexpected output: {}",
            cleaned.text
        );
        let restored = restore(&cleaned.text, &vault).expect("restoring");
        assert_eq!(restored.text, text);
        assert_eq!(restored.unknown, 0);
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
    fn a_sequence_too_large_to_parse_is_left_untouched() {
        let fixture = Fixture::new();
        let text = "See [[PHONE_99999999999999999999]] please.";
        let report = restore(text, &fixture.vault).expect("restoring");
        assert_eq!(report.text, text, "an unparseable sequence must stay put");
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
        let detector = Detector::new(&fixture.config).expect("detector");
        let cleaned = clean(
            "Line one\n\nMail: a@example.com\n",
            &detector,
            &mut fixture.vault,
        )
        .expect("cleaning");
        let restored = restore(&cleaned.text, &fixture.vault).expect("restoring");
        assert_eq!(restored.text, "Line one\n\nMail: a@example.com\n");
    }
}
