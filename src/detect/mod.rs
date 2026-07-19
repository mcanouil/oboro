//! Detection of sensitive entities in text.
//!
//! Layers run in order of increasing cost: deterministic rules first, then
//! (from later phases) a local NER model and an optional local LLM. Every
//! layer produces [`Span`]s over the same text, which [`merge`] reconciles.

pub mod merge;
pub mod rules;

use std::fmt;

/// A category of sensitive information.
///
/// The variant determines the placeholder tag written into sanitised output,
/// so renaming a variant changes every existing vault's placeholders.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntityKind {
    Person,
    Organisation,
    Address,
    Phone,
    Email,
    Iban,
    CreditCard,
    Siren,
    Siret,
    IpAddress,
    /// A user-defined pattern from `hush.toml`, such as a contract number.
    Custom(String),
}

impl EntityKind {
    /// The uppercase tag used in placeholders, for example `PHONE` in
    /// `[[PHONE_1]]`.
    #[must_use]
    pub fn tag(&self) -> String {
        match self {
            Self::Person => "PERSON".to_owned(),
            Self::Organisation => "ORG".to_owned(),
            Self::Address => "ADDRESS".to_owned(),
            Self::Phone => "PHONE".to_owned(),
            Self::Email => "EMAIL".to_owned(),
            Self::Iban => "IBAN".to_owned(),
            Self::CreditCard => "CARD".to_owned(),
            Self::Siren => "SIREN".to_owned(),
            Self::Siret => "SIRET".to_owned(),
            Self::IpAddress => "IP".to_owned(),
            Self::Custom(name) => sanitise_tag(name),
        }
    }

    /// Reconstructs a kind from a placeholder tag.
    ///
    /// Unknown tags are treated as custom kinds so that a vault written by a
    /// newer version still restores under an older one.
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "PERSON" => Self::Person,
            "ORG" => Self::Organisation,
            "ADDRESS" => Self::Address,
            "PHONE" => Self::Phone,
            "EMAIL" => Self::Email,
            "IBAN" => Self::Iban,
            "CARD" => Self::CreditCard,
            "SIREN" => Self::Siren,
            "SIRET" => Self::Siret,
            "IP" => Self::IpAddress,
            other => Self::Custom(other.to_owned()),
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.tag())
    }
}

/// Normalises a user-supplied pattern name into a placeholder-safe tag.
fn sanitise_tag(name: &str) -> String {
    let tag: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let tag = tag.trim_matches('_').to_owned();
    if tag.is_empty() {
        "CUSTOM".to_owned()
    } else {
        tag
    }
}

/// A detected entity occupying a byte range of the source text.
///
/// `start` and `end` are byte offsets and always fall on character
/// boundaries, so slicing the source with them cannot panic.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: EntityKind,
    pub text: String,
    /// Detector confidence in `0.0..=1.0`; deterministic rules report `1.0`.
    pub confidence: f32,
}

impl Span {
    #[must_use]
    pub fn new(start: usize, end: usize, kind: EntityKind, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            kind,
            text: text.into(),
            confidence: 1.0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether two spans share at least one byte.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_round_trip_through_from_tag() {
        let kinds = [
            EntityKind::Person,
            EntityKind::Organisation,
            EntityKind::Address,
            EntityKind::Phone,
            EntityKind::Email,
            EntityKind::Iban,
            EntityKind::CreditCard,
            EntityKind::Siren,
            EntityKind::Siret,
            EntityKind::IpAddress,
            EntityKind::Custom("CONTRACT".to_owned()),
        ];
        for kind in kinds {
            assert_eq!(EntityKind::from_tag(&kind.tag()), kind);
        }
    }

    #[test]
    fn custom_names_are_sanitised_into_tags() {
        assert_eq!(
            EntityKind::Custom("contract number".to_owned()).tag(),
            "CONTRACT_NUMBER"
        );
        assert_eq!(EntityKind::Custom("!!!".to_owned()).tag(), "CUSTOM");
    }

    #[test]
    fn overlap_is_exclusive_at_the_boundary() {
        let a = Span::new(0, 5, EntityKind::Email, "abcde");
        let b = Span::new(5, 9, EntityKind::Email, "fghi");
        let c = Span::new(4, 9, EntityKind::Email, "efghi");
        assert!(!a.overlaps(&b), "adjacent spans must not overlap");
        assert!(a.overlaps(&c));
        assert!(c.overlaps(&a));
    }
}
