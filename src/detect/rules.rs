//! Deterministic pattern recognisers.
//!
//! No language is declared or detected: every recogniser runs on every
//! document, and the ones that need words rather than digits carry the
//! vocabulary of several languages at once.
//!
//! Every recogniser pairs a permissive regex with a validator, so structural
//! checks (Luhn, IBAN mod-97, `libphonenumber`) reject the bulk of the false
//! positives a regex alone would produce.
//!
//! Where a trade-off remains, these recognisers favour recall: over-redaction
//! is recoverable through the allowlist in `oboro.toml`, whereas a missed
//! entity is a leak.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::LazyLock;

use phonenumber::country::Id;
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

/// Street addresses, in the word orders written across languages.
///
/// The type-first branch (`12 rue de la Paix`, `3 via Roma`) carries the
/// romance vocabulary. The type-last branch (`10 Downing Street`) carries the
/// germanic one and demands a capitalised name, which is what keeps `3 way
/// split` out. The last two read the German and Dutch habit of welding the
/// type onto the name, with the number after it (`Hauptstraße 5`) or before
/// (`12 Kerkstraat`).
///
/// Abbreviations are only accepted with their full stop, since a bare `st` or
/// `dr` after a number appears in ordinary prose too.
static STREET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)
        \b\d{{1,4}} \s* (?i:bis|ter|quater)? ,? \s+ (?i:{TYPE_FIRST}) \b [^\r\n,;.]{{2,60}}
        |
        \b\d{{1,4}}[a-zA-Z]? ,? \s+ (?:\p{{Lu}}[\p{{L}}'’\-]* \s+){{1,4}}
            (?: (?i:{TYPE_LAST})\b | (?i:{TYPE_ABBREVIATED})\. )
        |
        \b\p{{Lu}}[\p{{L}}\-]{{2,}} (?i:{TYPE_COMPOUND}) \s+ \d{{1,4}}[a-zA-Z]?\b
        |
        \b\d{{1,4}}[a-zA-Z]? ,? \s+ \p{{Lu}}[\p{{L}}\-]{{2,}} (?i:{TYPE_COMPOUND}) \b
        "
    ))
    .expect("street pattern is valid")
});

/// Street types written before the name.
const TYPE_FIRST: &str = r"rue|avenue|av\.|boulevard|bd\.?|chemin|place|impasse|all[ée]e|route|quai|cours|square|villa|passage|voie|sentier|via|viale|piazza|piazzale|corso|strada|calle|avenida|avda\.?|plaza|paseo|carrera|rua|travessa|pra[çc]a";

/// Street types written after the name, spelled out in full.
const TYPE_LAST: &str = r"street|road|avenue|lane|drive|way|court|close|crescent|terrace|square|boulevard|stra[ßs]se|weg|gasse|platz|allee|straat|laan|plein|gatan|gata|vej|vei";

/// Street types written after the name and abbreviated, so requiring a stop.
const TYPE_ABBREVIATED: &str = r"st|rd|ave|ln|dr|ct|blvd|str";

/// Street types welded onto the end of the name.
const TYPE_COMPOUND: &str = r"stra[ßs]e|str\.|weg|gasse|platz|allee|straat|laan|plein|gatan|vej";

/// Postcodes, in every shape that can be told apart from ordinary text.
///
/// A bare four-digit postcode followed by a place name is deliberately absent:
/// it cannot be distinguished from `2024 January`, and the false positives
/// would fall on every document that mentions a year.
static POSTCODE_CITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        # Five digits then a place: fr, de, es, it, tr.
        \b\d{5}\s+ [A-ZÀ-ÖØ-Þ][\p{L}'’\-]+ (?:[\x20\-][A-ZÀ-ÖØ-Þ]?[\p{L}'’\-]+){0,3} \b
        |
        # Four digits, two letters, then a place: nl.
        \b\d{4}\s?[A-Z]{2}\s+ [A-ZÀ-ÖØ-Þ][\p{L}'’\-]+ (?:[\x20\-][A-ZÀ-ÖØ-Þ]?[\p{L}'’\-]+){0,3} \b
        |
        # Identifying on their own: gb, then ca.
        \b[A-Z]{1,2}\d[A-Z\d]?\s?\d[A-Z]{2}\b
        |
        \b[A-Z]\d[A-Z]\s?\d[A-Z]\d\b
        |
        # Place first, then the code: us.
        \b[A-ZÀ-ÖØ-Þ][\p{L}'’\-]+(?:\x20[A-ZÀ-ÖØ-Þ][\p{L}'’\-]+){0,2},\s?[A-Z]{2}\s+\d{5}(?:-\d{4})?\b
        ",
    )
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
            |m| is_valid_phone(m, &self.config.regions),
        );
        push_matches(&mut spans, text, &IPV4, &EntityKind::IpAddress, |m| {
            Ipv4Addr::from_str(m.trim()).is_ok()
        });
        push_matches(&mut spans, text, &IPV6, &EntityKind::IpAddress, |m| {
            Ipv6Addr::from_str(m.trim()).is_ok()
        });
        push_matches(&mut spans, text, &STREET, &EntityKind::Address, |_| true);
        push_matches(
            &mut spans,
            text,
            &POSTCODE_CITY,
            &EntityKind::Address,
            |_| true,
        );

        self.push_custom(&mut spans, text);
        self.push_denylist(&mut spans, text);
        self.drop_allowlisted(&mut spans);

        spans
    }

    /// Applies user-defined patterns from `oboro.toml`.
    ///
    /// These are declared deliberately by the user, so they rank as exact.
    fn push_custom(&self, spans: &mut Vec<Span>, text: &str) {
        for pattern in &self.config.patterns {
            let kind = EntityKind::Custom(pattern.name.clone());
            push_matches(spans, text, &pattern.regex, &kind, |_| true);
        }
    }

    /// Applies literal terms the user always wants redacted, such as a known
    /// client list. Matching is whole-word, and case-insensitive unless the
    /// term sets `case_sensitive`.
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

/// Validates a candidate through `libphonenumber`.
///
/// No region is tried first, which catches every `+` prefixed number whatever
/// the configuration holds. Each configured region is then tried in turn, so
/// listing more regions widens what national formats are read without any of
/// them being required.
fn is_valid_phone(candidate: &str, regions: &[Id]) -> bool {
    let trimmed = candidate.trim();
    // Reject runs that are clearly something else, such as long account codes.
    let digit_count = digits_of(trimmed).len();
    if !(7..=15).contains(&digit_count) {
        return false;
    }

    std::iter::once(None)
        .chain(regions.iter().copied().map(Some))
        .any(|region| {
            phonenumber::parse(region, trimmed).is_ok_and(|number| phonenumber::is_valid(&number))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration with fixed regions, so these tests do not depend on the
    /// locale of the machine running them.
    fn config_for(regions: &[Id]) -> Config {
        let mut config = Config::default();
        config.regions = regions.to_vec();
        config
    }

    fn detect(text: &str) -> Vec<Span> {
        let config = config_for(&[Id::FR]);
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
    fn a_street_address_stops_before_a_carriage_return() {
        // A Windows-authored document uses CRLF; the address must end at the
        // line break rather than trailing a literal carriage return into the
        // stored value.
        let found = kinds_of(
            "Livraison au 12 rue de la Paix\r\nMerci.",
            &EntityKind::Address,
        );
        assert!(
            found.iter().any(|f| f.contains("rue de la Paix")),
            "got {found:?}"
        );
        assert!(
            found.iter().all(|f| !f.contains('\r')),
            "an address must not include the carriage return: {found:?}"
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

    /// The address recognisers carry several languages at once, with no
    /// language declared anywhere.
    #[test]
    fn finds_street_addresses_in_several_languages() {
        for (text, expected) in [
            (
                "Delivery to 10 Downing Street, London.",
                "10 Downing Street",
            ),
            ("Adresse: Hauptstraße 5, Berlin.", "Hauptstraße 5"),
            ("Wohnt in 5 Hauptstraße heute.", "5 Hauptstraße"),
            ("Bezorging op Kerkstraat 12 vandaag.", "Kerkstraat 12"),
            ("Vive en 3 Calle Mayor ahora.", "3 Calle Mayor"),
            ("Consegna al 7 via Roma domani.", "7 via Roma"),
            ("Send to 221B Baker St. tomorrow.", "221B Baker St."),
            ("Mora na 4 Kungsgatan idag.", "4 Kungsgatan"),
        ] {
            let found = kinds_of(text, &EntityKind::Address);
            assert!(
                found.iter().any(|f| f.contains(expected)),
                "{expected} was not found in {text:?}, got {found:?}"
            );
        }
    }

    #[test]
    fn finds_postcodes_in_several_formats() {
        for (text, expected) in [
            ("Office at SW1A 1AA in London.", "SW1A 1AA"),
            ("Bureau à K1A 0B6 au Canada.", "K1A 0B6"),
            ("Adres: 1234 AB Amsterdam hier.", "1234 AB Amsterdam"),
            (
                "Lives in Springfield, IL 62704 now.",
                "Springfield, IL 62704",
            ),
        ] {
            let found = kinds_of(text, &EntityKind::Address);
            assert!(
                found.iter().any(|f| f.contains(expected)),
                "{expected} was not found in {text:?}, got {found:?}"
            );
        }
    }

    /// A capitalised name is required before a type-last street word, and a
    /// four-digit postcode is not read at all, both to keep ordinary prose out.
    #[test]
    fn ordinary_prose_is_not_read_as_an_address() {
        for text in [
            "We agreed on a 3 way split of the invoice.",
            "The report covers 2024 January onwards.",
            "Invoice 12 was sent by way of post.",
        ] {
            let found = kinds_of(text, &EntityKind::Address);
            assert!(
                found.is_empty(),
                "{text:?} was read as an address: {found:?}"
            );
        }
    }

    #[test]
    fn an_international_number_is_read_with_no_regions_at_all() {
        let config = config_for(&[]);
        let spans =
            Rules::new(&config).detect("Appelez le +33 1 42 68 53 00 ou le 06 12 34 56 78.");
        let phones: Vec<String> = spans
            .into_iter()
            .filter(|s| s.kind == EntityKind::Phone)
            .map(|s| s.text)
            .collect();
        assert_eq!(
            phones.len(),
            1,
            "only the international number can be read without a region: {phones:?}"
        );
        assert!(phones[0].contains("+33"));
    }

    #[test]
    fn listing_more_regions_reads_more_national_formats() {
        let config = config_for(&[Id::FR, Id::GB]);
        let spans = Rules::new(&config).detect("Call 07911 123456 or 06 12 34 56 78 today.");
        let phones = spans.iter().filter(|s| s.kind == EntityKind::Phone).count();
        assert_eq!(phones, 2, "both national formats must be read");

        let config = config_for(&[Id::FR]);
        let spans = Rules::new(&config).detect("Call 07911 123456 today.");
        assert!(
            !spans.iter().any(|s| s.kind == EntityKind::Phone),
            "a British number is not a valid French one"
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
            let config = config_for(&[Id::FR]);
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
