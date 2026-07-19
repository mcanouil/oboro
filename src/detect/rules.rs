//! Deterministic pattern recognisers for French and English documents.
//!
//! Every recogniser pairs a permissive regex with a validator, so structural
//! checks (Luhn, IBAN mod-97, `libphonenumber`) reject the bulk of the false
//! positives a regex alone would produce.
//!
//! Where a trade-off remains, these recognisers favour recall: over-redaction
//! is recoverable through the allowlist in `hush.toml`, whereas a missed
//! entity is a leak.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;

use super::{EntityKind, Span};
use crate::config::Config;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9](?:[a-z0-9\-]*[a-z0-9])?(?:\.[a-z0-9\-]+)*\.[a-z]{2,}\b",
    )
    .expect("email pattern is valid")
});

/// Candidate digit runs that `libphonenumber` then accepts or rejects.
static PHONE_CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+\d{1,3}[\s.\-]?)?(?:\(\d{1,4}\)[\s.\-]?)?\d(?:[\s.\-]?\d){6,14}")
        .expect("phone pattern is valid")
});

static IBAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[a-z]{2}\d{2}(?:[ ]?[a-z0-9]{4}){2,7}(?:[ ]?[a-z0-9]{1,3})?\b")
        .expect("iban pattern is valid")
});

static CARD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d(?:[ \-]?\d){12,18}\b").expect("card pattern is valid"));

static SIRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{3}[ ]?\d{3}[ ]?\d{3}[ ]?\d{5}\b").expect("siret pattern is valid")
});

static SIREN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{3}[ ]?\d{3}[ ]?\d{3}\b").expect("siren pattern is valid"));

static IPV4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,3}(?:\.\d{1,3}){3}\b").expect("ipv4 pattern is valid"));

static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:[a-f0-9]{0,4}:){2,7}[a-f0-9]{0,4}\b").expect("ipv6 pattern is valid")
});

/// French street addresses: a number, an optional `bis`/`ter`, a street type,
/// then the street name up to a line or clause break.
static FR_STREET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b\d{1,4}\s*(?:bis|ter|quater)?[,]?\s+(?:rue|avenue|av\.|boulevard|bd\.?|chemin|place|impasse|all[ée]e|route|quai|cours|square|villa|passage|voie|sentier)\b[^\n,;.]{2,60}",
    )
    .expect("street pattern is valid")
});

/// A French postcode followed by a capitalised commune name.
static FR_POSTCODE_CITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{5}\s+[A-ZÀ-ÖØ-Þ][\p{L}'’\-]+(?:[ \-][A-ZÀ-ÖØ-Þ]?[\p{L}'’\-]+){0,3}\b")
        .expect("postcode pattern is valid")
});

/// The rules layer, compiled once against a [`Config`].
pub struct Rules<'a> {
    config: &'a Config,
}

impl<'a> Rules<'a> {
    #[must_use]
    pub fn new(config: &'a Config) -> Self {
        Self { config }
    }

    /// Finds every entity the deterministic layer recognises.
    ///
    /// Spans may overlap; reconciling them is [`super::merge`]'s job.
    #[must_use]
    pub fn detect(&self, text: &str) -> Vec<Span> {
        let mut spans = Vec::new();

        push_matches(&mut spans, text, &EMAIL, &EntityKind::Email, |_| true);
        push_matches(&mut spans, text, &IBAN, &EntityKind::Iban, is_valid_iban);
        push_matches(&mut spans, text, &CARD, &EntityKind::CreditCard, |m| {
            let digits = digits_of(m);
            (13..=19).contains(&digits.len()) && luhn_valid(&digits)
        });
        push_matches(&mut spans, text, &SIRET, &EntityKind::Siret, |m| {
            let digits = digits_of(m);
            // A real SIRET is a Luhn-valid SIREN followed by a Luhn-valid
            // whole; requiring both rejects most incidental digit runs.
            digits.len() == 14 && luhn_valid(&digits) && luhn_valid(&digits[..9])
        });
        push_matches(&mut spans, text, &SIREN, &EntityKind::Siren, |m| {
            let digits = digits_of(m);
            digits.len() == 9 && luhn_valid(&digits)
        });
        push_matches(
            &mut spans,
            text,
            &PHONE_CANDIDATE,
            &EntityKind::Phone,
            |m| is_valid_phone(m, &self.config.default_region),
        );
        push_matches(&mut spans, text, &IPV4, &EntityKind::IpAddress, |m| {
            Ipv4Addr::from_str(m.trim()).is_ok()
        });
        push_matches(&mut spans, text, &IPV6, &EntityKind::IpAddress, |m| {
            Ipv6Addr::from_str(m.trim()).is_ok()
        });
        push_matches(&mut spans, text, &FR_STREET, &EntityKind::Address, |_| true);
        push_matches(
            &mut spans,
            text,
            &FR_POSTCODE_CITY,
            &EntityKind::Address,
            |_| true,
        );

        self.push_custom(&mut spans, text);
        self.push_denylist(&mut spans, text);
        self.drop_allowlisted(&mut spans);

        spans
    }

    /// Applies user-defined patterns from `hush.toml`.
    ///
    /// These are declared deliberately by the user, so they rank as exact.
    fn push_custom(&self, spans: &mut Vec<Span>, text: &str) {
        for pattern in &self.config.patterns {
            let kind = EntityKind::Custom(pattern.name.clone());
            push_matches(spans, text, &pattern.regex, &kind, |_| true);
        }
    }

    /// Applies literal terms the user always wants redacted, such as a known
    /// client list. Matching is case-insensitive and whole-word.
    fn push_denylist(&self, spans: &mut Vec<Span>, text: &str) {
        for entry in &self.config.denylist {
            push_matches(spans, text, &entry.regex, &entry.kind, |_| true);
        }
    }

    /// Removes detections the user has explicitly marked as safe.
    fn drop_allowlisted(&self, spans: &mut Vec<Span>) {
        if self.config.allowlist.is_empty() {
            return;
        }
        spans.retain(|span| !self.config.is_allowlisted(&span.text));
    }
}

/// Pushes every regex match that clears `validate` onto `spans`.
fn push_matches(
    spans: &mut Vec<Span>,
    text: &str,
    regex: &Regex,
    kind: &EntityKind,
    validate: impl Fn(&str) -> bool,
) {
    for m in regex.find_iter(text) {
        let matched = m.as_str();
        if validate(matched) {
            spans.push(Span::new(m.start(), m.end(), kind.clone(), matched));
        }
    }
}

/// Keeps only ASCII digits, discarding the separators humans write.
fn digits_of(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

/// The Luhn checksum used by payment cards, SIREN and SIRET.
#[must_use]
pub fn luhn_valid(digits: &str) -> bool {
    if digits.len() < 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(index, byte)| {
            let digit = u32::from(byte - b'0');
            if index % 2 == 1 {
                let doubled = digit * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                digit
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// The ISO 13616 mod-97 check, computed iteratively to avoid big integers.
#[must_use]
pub fn is_valid_iban(candidate: &str) -> bool {
    let compact: String = candidate
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();

    if !(15..=34).contains(&compact.len()) || !compact.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let (head, tail) = compact.split_at(4);
    if !head[..2].bytes().all(|b| b.is_ascii_uppercase())
        || !head[2..].bytes().all(|b| b.is_ascii_digit())
    {
        return false;
    }

    let mut remainder: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let value = if c.is_ascii_digit() {
            u32::from(c as u8 - b'0')
        } else {
            u32::from(c as u8 - b'A') + 10
        };
        // Two-digit letter values must be folded in one digit at a time.
        remainder = if value >= 10 {
            (remainder * 100 + value) % 97
        } else {
            (remainder * 10 + value) % 97
        };
    }
    remainder == 1
}

/// Validates a candidate through `libphonenumber`, trying the configured
/// region for national formats and no region for `+` prefixed numbers.
fn is_valid_phone(candidate: &str, default_region: &str) -> bool {
    let trimmed = candidate.trim();
    // Reject runs that are clearly something else, such as long account codes.
    let digit_count = digits_of(trimmed).len();
    if !(7..=15).contains(&digit_count) {
        return false;
    }

    let region = phonenumber::country::Id::from_str(default_region).ok();
    if let Ok(number) = phonenumber::parse(region, trimmed) {
        return phonenumber::is_valid(&number);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(text: &str) -> Vec<Span> {
        let config = Config::default();
        Rules::new(&config).detect(text)
    }

    fn kinds_of(text: &str, kind: &EntityKind) -> Vec<String> {
        detect(text)
            .into_iter()
            .filter(|s| &s.kind == kind)
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn luhn_accepts_known_valid_numbers() {
        assert!(luhn_valid("4242424242424242"));
        assert!(luhn_valid("79927398713"));
        assert!(luhn_valid("552100554")); // SIREN of Danone.
    }

    #[test]
    fn luhn_rejects_corrupted_numbers() {
        assert!(!luhn_valid("4242424242424243"));
        assert!(!luhn_valid("79927398710"));
        assert!(!luhn_valid(""));
        assert!(!luhn_valid("4"));
        assert!(!luhn_valid("42x2"));
    }

    #[test]
    fn iban_accepts_valid_examples() {
        assert!(is_valid_iban("FR1420041010050500013M02606"));
        assert!(is_valid_iban("FR14 2004 1010 0505 0001 3M02 606"));
        assert!(is_valid_iban("GB82 WEST 1234 5698 7654 32"));
        assert!(is_valid_iban("DE89370400440532013000"));
    }

    #[test]
    fn iban_rejects_wrong_checksum_and_shape() {
        assert!(!is_valid_iban("FR1420041010050500013M02607"));
        assert!(!is_valid_iban("GB82WEST12345698765433"));
        assert!(!is_valid_iban("1234567890123456"));
        assert!(!is_valid_iban("FR14"));
        assert!(!is_valid_iban(""));
    }

    #[test]
    fn finds_emails() {
        let found = kinds_of(
            "Write to jean.dupont@example.com or sales@sub.example.co.uk today.",
            &EntityKind::Email,
        );
        assert_eq!(
            found,
            ["jean.dupont@example.com", "sales@sub.example.co.uk"]
        );
    }

    #[test]
    fn finds_french_and_international_phone_numbers() {
        let found = kinds_of(
            "Appelez le 06 12 34 56 78 ou le +33 1 42 68 53 00.",
            &EntityKind::Phone,
        );
        assert_eq!(found.len(), 2, "expected both numbers, got {found:?}");
    }

    #[test]
    fn ignores_numbers_that_are_not_valid_phones() {
        let found = kinds_of("Reference 0000000 and 1234567.", &EntityKind::Phone);
        assert!(found.is_empty(), "unexpected phone matches: {found:?}");
    }

    #[test]
    fn finds_iban_in_running_text() {
        let found = kinds_of(
            "Virement sur FR14 2004 1010 0505 0001 3M02 606 avant vendredi.",
            &EntityKind::Iban,
        );
        assert_eq!(found, ["FR14 2004 1010 0505 0001 3M02 606"]);
    }

    #[test]
    fn finds_french_street_address() {
        let found = kinds_of(
            "Livraison au 12 bis rue de la Paix\nMerci.",
            &EntityKind::Address,
        );
        assert!(
            found.iter().any(|f| f.contains("rue de la Paix")),
            "got {found:?}"
        );
    }

    #[test]
    fn finds_postcode_and_city() {
        let found = kinds_of("Adresse: 59000 Lille, France.", &EntityKind::Address);
        assert!(
            found.iter().any(|f| f.contains("59000 Lille")),
            "got {found:?}"
        );
    }

    #[test]
    fn finds_ip_addresses_and_rejects_invalid_octets() {
        let found = kinds_of("Hosts 192.168.1.10 and 999.1.1.1.", &EntityKind::IpAddress);
        assert_eq!(found, ["192.168.1.10"]);
    }

    /// Resolved detection, mirroring what the pipeline actually writes.
    fn resolved(text: &str) -> Vec<Span> {
        crate::detect::merge::resolve(detect(text))
    }

    #[test]
    fn a_siren_outranks_the_phone_number_it_resembles() {
        let spans = resolved("Immatriculé sous le SIREN 123456782 depuis 2020.");
        let siren = spans
            .iter()
            .find(|s| s.text.contains("123456782"))
            .expect("the SIREN must be detected");
        assert_eq!(
            siren.kind,
            EntityKind::Siren,
            "a nine-digit SIREN must not be labelled as a phone number"
        );
    }

    #[test]
    fn a_siret_outranks_the_card_number_it_resembles() {
        let spans = resolved("SIRET 12345678200002 au registre.");
        let siret = spans
            .iter()
            .find(|s| s.text.contains("12345678200002"))
            .expect("the SIRET must be detected");
        assert_eq!(siret.kind, EntityKind::Siret);
    }

    #[test]
    fn a_siret_requires_both_checksums() {
        // Luhn-valid as a whole, but its first nine digits are not a valid
        // SIREN, so it is not a SIRET.
        assert!(
            kinds_of("Numéro 39525498100008 ici.", &EntityKind::Siret).is_empty(),
            "a SIRET whose SIREN prefix fails Luhn must be rejected"
        );
    }

    #[test]
    fn card_numbers_are_still_recognised_as_cards() {
        let spans = resolved("Carte 4242 4242 4242 4242 enregistrée.");
        let card = spans
            .iter()
            .find(|s| s.text.contains("4242"))
            .expect("the card must be detected");
        assert_eq!(card.kind, EntityKind::CreditCard);
    }

    proptest::proptest! {
        /// Luhn's purpose is catching single-digit typos, so altering one
        /// digit of a valid number must always invalidate it.
        #[test]
        fn luhn_rejects_any_single_digit_typo(
            digits in proptest::collection::vec(0u8..10, 8..20),
            position in 0usize..8,
            shift in 1u8..10,
        ) {
            let mut number: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
            // Fix the final digit so the number is Luhn-valid to begin with.
            let check = (0..10u8)
                .find(|candidate| {
                    let mut trial = number.clone();
                    trial.pop();
                    trial.push(char::from(b'0' + candidate));
                    luhn_valid(&trial)
                })
                .expect("some check digit always makes Luhn hold");
            number.pop();
            number.push(char::from(b'0' + check));
            proptest::prop_assert!(luhn_valid(&number));

            let position = position % number.len();
            let original = number.as_bytes()[position] - b'0';
            let replacement = (original + shift) % 10;
            proptest::prop_assume!(replacement != original);
            let mut corrupted: Vec<u8> = number.into_bytes();
            corrupted[position] = b'0' + replacement;
            let corrupted = String::from_utf8(corrupted).expect("still ASCII");
            proptest::prop_assert!(!luhn_valid(&corrupted), "typo went undetected in {corrupted}");
        }

        /// The validators must never panic, whatever the input.
        #[test]
        fn validators_tolerate_arbitrary_input(text in ".{0,64}") {
            let _ = luhn_valid(&text);
            let _ = is_valid_iban(&text);
        }

        /// Detection must never panic and must stay inside the text.
        #[test]
        fn detection_yields_spans_within_the_text(text in ".{0,256}") {
            let config = Config::default();
            for span in Rules::new(&config).detect(&text) {
                proptest::prop_assert!(span.end <= text.len());
                proptest::prop_assert!(span.start <= span.end);
                proptest::prop_assert_eq!(&text[span.start..span.end], span.text.as_str());
            }
        }
    }

    #[test]
    fn empty_text_yields_no_spans() {
        assert!(detect("").is_empty());
    }

    #[test]
    fn text_without_entities_yields_no_spans() {
        assert!(detect("The quick brown fox jumps over the lazy dog.").is_empty());
    }
}
