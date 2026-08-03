//! The test that matters most: sanitised output must not contain any value
//! the fixture planted.
//!
//! Every other guarantee is a convenience. This one is the product.

mod support;

use support::Workspace;

/// Values planted in `testdata/contract.txt` that must never survive `clean`.
///
/// Formatted and compact spellings are both listed: a detector that matches
/// the spaced form but leaves a compact one behind is still a leak.
const PLANTED: &[&str] = &[
    "jean.dupont@acme-consulting.example",
    "marie.martin@globex.example",
    "06 12 34 56 78",
    "0612345678",
    "+33 1 42 68 53 00",
    "FR14 2004 1010 0505 0001 3M02 606",
    "FR1420041010050500013M02606",
    "4242 4242 4242 4242",
    "4242424242424242",
    "12345678200002",
    "123456782",
    "192.168.14.201",
    "12 bis rue de la Paix",
    "8 avenue des Champs-Élysées",
    "75002 Paris",
    "75008 Paris",
    "Acme Consulting SARL",
    "Globex Industries",
    "Jean Dupont",
    "CT-874512",
];

/// Every fixture any build can read, so a converter cannot be added without
/// the leak test covering it.
const DOCUMENTS: &[&str] = &[
    "contract.txt",
    "contract.docx",
    "contract.odt",
    "clients.xlsx",
    "clients.csv",
    "clients.tsv",
    "invoice.pdf",
    "slides.pptx",
    "message.eml",
];

/// The fixtures that only a build with the `ocr` feature can read: an image,
/// and the two ways a scanner stores a page inside a PDF.
///
/// Text recovered by recognition is held to the same standard as text read
/// directly. It reaches the model the same way, so it leaks the same way.
#[cfg(feature = "ocr")]
const OCR_DOCUMENTS: &[&str] = &["scan.png", "scan.pdf", "scan-fax.pdf"];

#[cfg(not(feature = "ocr"))]
const OCR_DOCUMENTS: &[&str] = &[];

/// Every fixture this build can read.
fn readable() -> impl Iterator<Item = &'static &'static str> {
    DOCUMENTS.iter().chain(OCR_DOCUMENTS)
}

#[test]
fn no_planted_value_survives_cleaning() {
    for document in readable() {
        let workspace = Workspace::new();
        let cleaned = workspace.clean_fixture(document);

        let leaked: Vec<&str> = PLANTED
            .iter()
            .copied()
            .filter(|planted| cleaned.contains(planted))
            .collect();

        assert!(
            leaked.is_empty(),
            "{document} leaked {} value(s): {leaked:#?}\n\n--- output ---\n{cleaned}",
            leaked.len()
        );
    }
}

/// Piped text is held to the same standard as a file: an agent hook cleans
/// standard input, so a value that survives that path leaks just as surely as
/// one that survives a document.
///
/// Only the text fixture is used: standard input has no extension to sniff, so
/// it is read as text or markdown and never goes through a converter.
#[test]
fn no_planted_value_survives_cleaning_from_standard_input() {
    let workspace = Workspace::new();
    let original =
        std::fs::read_to_string(support::fixture("contract.txt")).expect("reading the fixture");
    let cleaned = workspace.clean_piped(&original);

    let leaked: Vec<&str> = PLANTED
        .iter()
        .copied()
        .filter(|planted| cleaned.contains(planted))
        .collect();
    assert!(
        leaked.is_empty(),
        "piped input leaked {} value(s): {leaked:#?}\n\n--- output ---\n{cleaned}",
        leaked.len()
    );

    assert_eq!(
        workspace.restore_piped(&cleaned),
        original,
        "restoring piped output must reproduce the original document exactly"
    );
}

/// Accented prose must survive conversion untouched. A reader that dropped
/// entity references would turn "Société" into "Socit", which is both wrong
/// in the output and no longer matches a denylisted name.
#[test]
fn accented_text_survives_document_conversion() {
    let workspace = Workspace::new();
    let cleaned = workspace.clean_fixture("contract.docx");
    for expected in ["Représenté", "Téléphone", "Référence"] {
        assert!(
            cleaned.contains(expected),
            "conversion mangled '{expected}':\n{cleaned}"
        );
    }
}

/// The values each fixture plants, as its converter actually produces them.
///
/// One list, used in both directions: [`no_planted_value_survives_cleaning`]
/// proves nothing here survived, and [`no_planted_value_is_lost_in_conversion`]
/// proves nothing here was lost on the way in. The leak test alone can only
/// see survival, since text a converter never produced matches nothing and so
/// passes silently, and loss is the failure `src/convert/mod.rs` names as its
/// own: output that looks sanitised without having been read.
///
/// Listing every value a fixture carries, rather than one per fixture, is what
/// catches partial loss: a converter that dropped everything past the first
/// paragraph would otherwise still pass.
///
/// The recognised fixtures carry a deliberately short list. Their text comes
/// back from Tesseract rather than from the document, so an assertion on a
/// mixed-case identifier would be pinning one version's recognition rather
/// than this code; the two plain names are what recognition gets right across
/// versions.
const PLANTED_IN: &[(&str, &[&str])] = &[
    (
        "contract.txt",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "FR14 2004 1010 0505 0001 3M02 606",
            "4242 4242 4242 4242",
            "12345678200002",
            "192.168.14.201",
            "12 bis rue de la Paix",
            "8 avenue des Champs-Élysées",
            "75002 Paris",
            "75008 Paris",
            "Acme Consulting SARL",
            "Globex Industries",
            "Jean Dupont",
            "CT-874512",
        ],
    ),
    (
        "contract.docx",
        &[
            "jean.dupont@acme-consulting.example",
            "06 12 34 56 78",
            "FR14 2004 1010 0505 0001 3M02 606",
            "12345678200002",
            "12 bis rue de la Paix",
            "75002 Paris",
            "Acme Consulting SARL",
            "Jean Dupont",
            "CT-874512",
        ],
    ),
    (
        "contract.odt",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "FR14 2004 1010 0505 0001 3M02 606",
            "4242 4242 4242 4242",
            "12345678200002",
            "192.168.14.201",
            "12 bis rue de la Paix",
            "8 avenue des Champs-Élysées",
            "75002 Paris",
            "Acme Consulting SARL",
            "Globex Industries",
            "Jean Dupont",
            "CT-874512",
        ],
    ),
    (
        "clients.xlsx",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "123456782",
            "Acme Consulting SARL",
            "Globex Industries",
        ],
    ),
    (
        "clients.csv",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "FR14 2004 1010 0505 0001 3M02 606",
            "12345678200002",
            "Acme Consulting SARL",
            "Globex Industries",
            "Jean Dupont",
        ],
    ),
    (
        "message.eml",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "12 bis rue de la Paix",
            "8 avenue des Champs-Élysées",
            "75002 Paris",
            "Acme Consulting SARL",
            "Globex Industries",
            "Jean Dupont",
            "CT-874512",
        ],
    ),
    (
        "clients.tsv",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "FR14 2004 1010 0505 0001 3M02 606",
            "12345678200002",
            "Acme Consulting SARL",
            "Globex Industries",
            "Jean Dupont",
        ],
    ),
    (
        "invoice.pdf",
        &[
            "jean.dupont@acme-consulting.example",
            "06 12 34 56 78",
            "FR14 2004 1010 0505 0001 3M02 606",
            "12 bis rue de la Paix",
            "75002 Paris",
            "Acme Consulting SARL",
            "Jean Dupont",
            "CT-874512",
        ],
    ),
    (
        "slides.pptx",
        &[
            "jean.dupont@acme-consulting.example",
            "marie.martin@globex.example",
            "06 12 34 56 78",
            "+33 1 42 68 53 00",
            "FR14 2004 1010 0505 0001 3M02 606",
            "4242 4242 4242 4242",
            "12345678200002",
            "12 bis rue de la Paix",
            "75002 Paris",
            "Acme Consulting SARL",
            "Globex Industries",
            "Jean Dupont",
            "CT-874512",
        ],
    ),
    ("scan.png", &["Acme Consulting SARL", "Jean Dupont"]),
    ("scan.pdf", &["Acme Consulting SARL", "Jean Dupont"]),
    ("scan-fax.pdf", &["Acme Consulting SARL", "Jean Dupont"]),
];

/// The values a fixture plants, or a failure naming the fixture that has none.
///
/// Failing rather than skipping is what stops a new format being added to
/// [`DOCUMENTS`] without anything asserting its text arrives whole.
fn planted_in(document: &str) -> &'static [&'static str] {
    PLANTED_IN
        .iter()
        .find(|(name, _)| *name == document)
        .map_or_else(
            || panic!("{document} has no planted values listed"),
            |(_, values)| *values,
        )
}

/// The counterpart to the leak test: every value a fixture plants must be in
/// the text its converter produces, before any detection runs.
///
/// A value the converter drops is absent, so it is never detected, never
/// redacted, and never in the output; the leak test passes on it precisely
/// because it was lost. A value the converter glues to its neighbour fails
/// this test too, since the planted spelling is no longer a substring of what
/// was read.
///
/// This reads the converters directly rather than going through `clean`, so a
/// failure points at the reader rather than at the pipeline.
#[test]
fn no_planted_value_is_lost_in_conversion() {
    for document in readable() {
        let text = oboro::convert::read(&support::fixture(document), &["eng".to_owned()])
            .unwrap_or_else(|error| panic!("reading {document}: {error:#}"))
            .into_parts()
            .into_iter()
            .map(|(_, part)| part)
            .collect::<Vec<_>>()
            .join("\n");

        let lost: Vec<&str> = planted_in(document)
            .iter()
            .copied()
            .filter(|planted| !text.contains(planted))
            .collect();

        assert!(
            lost.is_empty(),
            "{document} lost {} value(s) in conversion: {lost:#?}\n\n--- read ---\n{text}",
            lost.len()
        );
    }
}

#[test]
fn every_document_format_round_trips() {
    for document in readable() {
        let workspace = Workspace::new();
        let cleaned = workspace.clean_fixture(document);
        let restored = workspace.restore(&cleaned);

        for expected in planted_in(document) {
            assert!(
                restored.contains(expected),
                "{document} did not restore {expected:?}:\n{restored}"
            );
        }
        assert!(
            !restored.contains("[["),
            "{document} left placeholders behind after restoring:\n{restored}"
        );
    }
}

/// A document whose text cannot be read must fail rather than produce output
/// that looks sanitised but was never actually read.
#[test]
fn a_scanned_document_is_refused_rather_than_half_read() {
    let workspace = Workspace::new();
    let output = workspace
        .command()
        .arg("clean")
        .arg(support::fixture("scanned.pdf"))
        .arg("--stdout")
        .output()
        .expect("running oboro clean");

    assert!(!output.status.success(), "a scanned PDF must not succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Not "scanned", which the fixture's own path carries: the assertion would
    // pass on the filename alone and prove nothing about the explanation.
    assert!(
        stderr.contains("sanitised"),
        "the error must say why: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing may be written for a document that could not be read"
    );
}

#[test]
fn cleaning_is_stable_across_runs() {
    let workspace = Workspace::new();
    let first = workspace.clean_fixture("contract.txt");
    let second = workspace.clean_fixture("contract.txt");
    assert_eq!(
        first, second,
        "the same input and vault must produce identical output"
    );
}

#[test]
fn every_planted_value_round_trips_back() {
    let workspace = Workspace::new();
    let cleaned = workspace.clean_fixture("contract.txt");
    let restored = workspace.restore(&cleaned);
    let original =
        std::fs::read_to_string(support::fixture("contract.txt")).expect("reading the fixture");
    assert_eq!(
        restored, original,
        "restoring must reproduce the original document exactly"
    );
}

/// A workbook is written as one TSV per sheet, and neither the cell values
/// nor a PII-bearing sheet name may survive into the outputs or their names.
#[test]
fn a_workbook_leaks_nothing_through_sheet_content_or_names() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(
        &book,
        &[
            (
                "Jean Dupont",
                &[
                    &["name", "email"],
                    &["Jean Dupont", "jean.dupont@acme-consulting.example"],
                ],
            ),
            ("Notes", &[&["phone"], &["06 12 34 56 78"]]),
        ],
    );
    let out_dir = workspace.path().join("sanitised");

    let output = workspace
        .command()
        .arg("clean")
        .arg(&book)
        .arg("--config")
        .arg(support::fixture("oboro.toml"))
        .arg("--output")
        .arg(&out_dir)
        .output()
        .expect("running oboro clean");
    assert!(
        output.status.success(),
        "oboro clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut outputs: Vec<std::path::PathBuf> = std::fs::read_dir(&out_dir)
        .expect("reading the output directory")
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    outputs.sort();
    assert_eq!(
        outputs.len(),
        2,
        "each sheet must become its own file: {outputs:#?}"
    );

    for path in &outputs {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name");
        assert!(
            name.ends_with(".clean.tsv"),
            "a workbook output must be TSV: {name}"
        );
        assert!(
            !name.contains("Jean Dupont"),
            "the sheet name PII must not survive into a filename: {name}"
        );

        let cleaned = std::fs::read_to_string(path).expect("reading an output");
        let leaked: Vec<&str> = PLANTED
            .iter()
            .copied()
            .filter(|planted| cleaned.contains(planted))
            .collect();
        assert!(
            leaked.is_empty(),
            "{name} leaked {} value(s): {leaked:#?}\n\n--- output ---\n{cleaned}",
            leaked.len()
        );
    }
    assert!(
        outputs.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("PERSON_1"))
        }),
        "the PII sheet name must be replaced by its placeholder: {outputs:#?}"
    );
}

#[test]
fn allowlisted_values_are_preserved() {
    let workspace = Workspace::new();
    let cleaned = workspace.clean_fixture("contract.txt");
    assert!(
        cleaned.contains("Lille"),
        "an allowlisted value was redacted:\n{cleaned}"
    );
}
