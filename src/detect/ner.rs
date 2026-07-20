//! Zero-shot recognition of the entities no pattern can describe.
//!
//! Rules find things with structure: a checksum, a format, a prefix. A
//! person's name has none of that, and listing every client by hand does not
//! scale. This layer runs a `GLiNER` model locally to find them, and sends
//! nothing anywhere.
//!
//! Compiled only with the `ner` feature.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use gliner::model::input::text::TextInput;
use gliner::model::params::Parameters;
use gliner::model::{GLiNER, pipeline::span::SpanMode};
use orp::params::RuntimeParameters;

use super::{EntityKind, Span};
use crate::config::Config;

/// The entity types asked of the model.
///
/// `GLiNER` is zero-shot, so these are plain words rather than trained classes.
/// Each maps onto the placeholder its values will be given.
const LABELS: &[(&str, EntityKind)] = &[
    ("person", EntityKind::Person),
    ("organization", EntityKind::Organisation),
    ("company", EntityKind::Organisation),
    ("address", EntityKind::Address),
];

/// A loaded model, ready to read text.
pub struct Recogniser {
    model: GLiNER<SpanMode>,
    threshold: f32,
}

impl Recogniser {
    /// Loads the model from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the files cannot be loaded as a `GLiNER` model.
    pub fn load(model: &Path, tokenizer: &Path, threshold: f32) -> Result<Self> {
        // The library applies this threshold while decoding, before any of
        // our own filtering, so the two must agree or the lower one never
        // takes effect.
        let model = GLiNER::<SpanMode>::new(
            Parameters::default().with_threshold(threshold),
            RuntimeParameters::default(),
            tokenizer,
            model,
        )
        .map_err(|error| anyhow!("{error}"))
        .context("loading the recognition model")?;

        Ok(Self { model, threshold })
    }

    /// Finds names, organisations and addresses in `text`.
    ///
    /// # Errors
    ///
    /// Returns an error if inference fails.
    pub fn detect(&self, text: &str) -> Result<Vec<Span>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let labels: Vec<&str> = LABELS.iter().map(|(label, _)| *label).collect();
        let input = TextInput::from_str(&[text], &labels)
            .map_err(|error| anyhow!("{error}"))
            .context("preparing text for the recognition model")?;

        let output = self
            .model
            .inference(input)
            .map_err(|error| anyhow!("{error}"))
            .context("running the recognition model")?;

        let mut spans = Vec::new();
        for found in output.spans.into_iter().flatten() {
            if found.probability() < self.threshold {
                continue;
            }
            let Some(kind) = kind_for(found.class()) else {
                continue;
            };
            for (start, end) in locate(text, found.text(), found.offsets()) {
                spans.push(Span {
                    start,
                    end,
                    kind: kind.clone(),
                    text: text[start..end].to_owned(),
                    confidence: found.probability(),
                });
            }
        }
        Ok(spans)
    }
}

/// Builds a recogniser if the feature, the model and the configuration all
/// allow it, otherwise `None`.
///
/// # Errors
///
/// Returns an error only when the model is present but unusable. A missing
/// model is a normal state, handled by `models pull`.
pub fn load_if_available(config: &Config) -> Result<Option<Recogniser>> {
    if !config.ner_enabled || !crate::models::is_installed()? {
        return Ok(None);
    }
    let (model, tokenizer) = crate::models::paths()?;
    Recogniser::load(&model, &tokenizer, config.ner_threshold).map(Some)
}

fn kind_for(label: &str) -> Option<EntityKind> {
    LABELS
        .iter()
        .find(|(candidate, _)| *candidate == label)
        .map(|(_, kind)| kind.clone())
}

/// Resolves a detected entity to byte ranges in `text`.
///
/// The model reports offsets alongside the text it matched, but whether those
/// offsets count bytes or characters is its business, not ours. Rather than
/// assume, the reported range is accepted only when it actually holds the
/// reported text. Failing that, every literal occurrence is returned: an
/// entity found once is worth redacting everywhere it appears, and dropping
/// it because an offset did not line up would be a leak.
fn locate(text: &str, entity: &str, offsets: (usize, usize)) -> Vec<(usize, usize)> {
    if entity.is_empty() {
        return Vec::new();
    }

    let (start, end) = offsets;
    if end <= text.len()
        && text.is_char_boundary(start)
        && text.is_char_boundary(end)
        && text.get(start..end) == Some(entity)
    {
        return vec![(start, end)];
    }

    if let Some((start, end)) = char_range_to_bytes(text, start, end)
        && text.get(start..end) == Some(entity)
    {
        return vec![(start, end)];
    }

    text.match_indices(entity)
        .map(|(index, matched)| (index, index + matched.len()))
        .collect()
}

/// Translates a character range into a byte range on the same text.
fn char_range_to_bytes(text: &str, start_char: usize, end_char: usize) -> Option<(usize, usize)> {
    if start_char >= end_char {
        return None;
    }
    let mut boundaries = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()));

    let start = boundaries.nth(start_char)?;
    let end = boundaries.nth(end_char - start_char - 1)?;
    (start < end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_correct_byte_range_is_used_as_given() {
        let text = "Jean Dupont paye";
        assert_eq!(locate(text, "Jean Dupont", (0, 11)), [(0, 11)]);
    }

    #[test]
    fn character_offsets_are_translated_when_bytes_do_not_fit() {
        let text = "Société Générale paye";
        // "Générale" is characters 8..16, but bytes 9..18.
        let found = locate(text, "Générale", (8, 16));
        assert_eq!(found.len(), 1);
        let (start, end) = found[0];
        assert_eq!(&text[start..end], "Générale");
    }

    #[test]
    fn a_wrong_offset_falls_back_to_finding_the_text() {
        let text = "Contact: Jean Dupont today";
        // Offsets that point at the wrong place entirely.
        let found = locate(text, "Jean Dupont", (0, 11));
        assert_eq!(found.len(), 1);
        let (start, end) = found[0];
        assert_eq!(&text[start..end], "Jean Dupont");
        assert_eq!(start, 9);
    }

    #[test]
    fn a_repeated_entity_is_redacted_everywhere_it_appears() {
        let text = "Jean Dupont wrote. Ask Jean Dupont again.";
        let found = locate(text, "Jean Dupont", (999, 1010));
        assert_eq!(found.len(), 2, "both occurrences must be found");
        for (start, end) in found {
            assert_eq!(&text[start..end], "Jean Dupont");
        }
    }

    #[test]
    fn an_entity_that_is_not_in_the_text_yields_nothing() {
        assert!(locate("nothing here", "Jean Dupont", (0, 11)).is_empty());
        assert!(locate("text", "", (0, 0)).is_empty());
    }

    #[test]
    fn offsets_past_the_end_do_not_panic() {
        let text = "short";
        assert!(locate(text, "Jean", (100, 200)).is_empty());
    }

    #[test]
    fn every_label_maps_to_a_kind() {
        for (label, _) in LABELS {
            assert!(kind_for(label).is_some(), "{label} has no kind");
        }
        assert!(kind_for("vehicle").is_none());
    }

    #[test]
    fn labels_cover_the_entities_rules_cannot_find() {
        let kinds: Vec<EntityKind> = LABELS.iter().map(|(_, kind)| kind.clone()).collect();
        assert!(kinds.contains(&EntityKind::Person));
        assert!(kinds.contains(&EntityKind::Organisation));
    }
}

/// Tests needing the downloaded model.
///
/// Ignored by default: they need `oboro models pull` to have run, which
/// fetches several hundred megabytes. Run them after changing the labels,
/// the threshold, or the model itself:
///
/// ```text
/// cargo test --features ner -- --ignored --nocapture
/// ```
#[cfg(test)]
mod calibration {
    use super::*;

    fn recogniser() -> Recogniser {
        let (model, tokenizer) = crate::models::paths().expect("run `oboro models pull` first");
        Recogniser::load(
            &model,
            &tokenizer,
            crate::config::Config::default().ner_threshold,
        )
        .expect("loading the model")
    }

    fn found(spans: &[Span], text: &str) -> bool {
        spans.iter().any(|span| span.text == text)
    }

    #[test]
    #[ignore = "needs the downloaded model"]
    fn finds_names_and_companies_no_rule_could_describe() {
        let recogniser = recogniser();

        let spans = recogniser
            .detect("Marie Lefevre a rencontre le directeur de Sogexia Partners hier.")
            .expect("detecting");
        assert!(found(&spans, "Marie Lefevre"), "missed a person: {spans:?}");
        assert!(
            found(&spans, "Sogexia Partners"),
            "missed a company: {spans:?}"
        );

        let spans = recogniser
            .detect("Please forward the report to Sarah O'Connell at Northwind Trading Ltd.")
            .expect("detecting");
        assert!(
            found(&spans, "Sarah O'Connell"),
            "missed a person: {spans:?}"
        );
        assert!(
            found(&spans, "Northwind Trading Ltd"),
            "missed a company: {spans:?}"
        );
    }

    /// Recall is the guarantee this layer offers, and precision is not.
    ///
    /// A false positive and a real name sit within a couple of hundredths of
    /// each other: "The quick brown fox" scores 0.218 while "Thomas Bernard"
    /// inside a document scores 0.237. No threshold separates them, so the
    /// tool redacts both and the user reviews the result. This test pins the
    /// direction of that trade rather than pretending it is not there.
    #[test]
    #[ignore = "needs the downloaded model"]
    fn prose_may_be_over_redacted_and_that_is_the_safer_failure() {
        let recogniser = recogniser();
        let spans = recogniser
            .detect("Elle propose que Thomas Bernard reprenne le dossier.")
            .expect("detecting");
        assert!(
            found(&spans, "Thomas Bernard"),
            "recall is the guarantee: {spans:?}"
        );
    }

    #[test]
    #[ignore = "needs the downloaded model"]
    fn detections_carry_the_model_probability_not_a_constant() {
        let recogniser = recogniser();
        let spans = recogniser
            .detect("Thomas Bernard reprend le dossier Valmont Industries.")
            .expect("detecting");
        assert!(!spans.is_empty());
        for span in &spans {
            assert!(
                span.confidence > 0.0 && span.confidence < 1.0,
                "expected a real probability, got {}",
                span.confidence
            );
        }
    }
}
