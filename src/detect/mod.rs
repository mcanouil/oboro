//! Detection of sensitive entities in text.
//!
//! Layers run in order of increasing cost: deterministic rules first, then
//! (from later phases) a local NER model and an optional local LLM. Every
//! layer produces [`Span`]s over the same text, which [`merge`] reconciles.

pub mod merge;
#[cfg(feature = "ner")]
pub mod ner;
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
    /// A user-defined pattern from `oboro.toml`, such as a contract number.
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

    /// How specifically this kind identifies the bytes it matched.
    ///
    /// Some values are plausibly two things at once: a nine-digit SIREN is
    /// also a well-formed French phone number, and a fourteen-digit SIRET is
    /// a well-formed card number. When two equally long spans disagree, the
    /// kind that verified more structure should name the value.
    ///
    /// This is deliberately separate from [`Span::confidence`]. Specificity
    /// ranks *labels*; confidence estimates whether a detector is *right*.
    /// Conflating them would make a hand-tuned constant compete directly
    /// with a model's probability once the NER layer lands.
    #[must_use]
    pub fn specificity(&self) -> u8 {
        match self {
            // Declared by the user, who knows their own data.
            Self::Custom(_) => 6,
            // Checksummed or syntactically unambiguous.
            Self::Email | Self::Iban | Self::IpAddress | Self::Siret => 5,
            Self::CreditCard => 4,
            Self::Person | Self::Organisation | Self::Siren => 3,
            Self::Phone => 2,
            // Matched on shape alone.
            Self::Address => 1,
        }
    }

    /// Folds away formatting differences that should not produce a second
    /// placeholder for what a reader would call the same value.
    ///
    /// This belongs to the kind rather than to the vault: whether two
    /// spellings mean the same thing is a fact about phone numbers and
    /// IBANs, not about storage.
    #[must_use]
    pub fn normalise(&self, value: &str) -> String {
        let trimmed = value.trim();
        match self {
            Self::Phone => trimmed
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '+')
                .collect(),
            Self::Iban | Self::CreditCard | Self::Siren | Self::Siret => trimmed
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_uppercase())
                .collect(),
            _ => trimmed
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase(),
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
    /// How sure the detector is that this really is an entity, in
    /// `0.0..=1.0`.
    ///
    /// Deterministic rules report `1.0`: each one has already cleared a
    /// checksum or a parser, so it is not guessing. Which *kind* wins when
    /// two rules claim the same bytes is [`EntityKind::specificity`], not
    /// this.
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
    fn a_more_specific_kind_outranks_the_kind_it_resembles() {
        assert!(EntityKind::Siren.specificity() > EntityKind::Phone.specificity());
        assert!(EntityKind::Siret.specificity() > EntityKind::CreditCard.specificity());
        assert!(
            EntityKind::Custom("contract".to_owned()).specificity()
                > EntityKind::Address.specificity()
        );
    }

    #[test]
    fn normalisation_folds_formatting_per_kind() {
        assert_eq!(
            EntityKind::Phone.normalise("06 12 34 56 78"),
            EntityKind::Phone.normalise("0612345678")
        );
        assert_eq!(
            EntityKind::Iban.normalise("FR14 2004 1010"),
            EntityKind::Iban.normalise("fr1420041010")
        );
        assert_eq!(
            EntityKind::Person.normalise("  Jean   Dupont "),
            EntityKind::Person.normalise("JEAN DUPONT")
        );
    }

    #[test]
    fn a_custom_kind_does_not_inherit_phone_normalisation() {
        // Dispatching on the tag string would have folded this to digits,
        // because the sanitised tag of "phone" is PHONE.
        let custom = EntityKind::Custom("phone".to_owned());
        assert_eq!(custom.normalise("Ref +33 1234"), "ref +33 1234");
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
