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

/// Every readable fixture, so a converter cannot be added without the leak
/// test covering it.
const DOCUMENTS: &[&str] = &[
    "contract.txt",
    "contract.docx",
    "clients.xlsx",
    "clients.csv",
    "clients.tsv",
    "invoice.pdf",
];

#[test]
fn no_planted_value_survives_cleaning() {
    for document in DOCUMENTS {
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

#[test]
fn every_document_format_round_trips() {
    for document in DOCUMENTS {
        let workspace = Workspace::new();
        let cleaned = workspace.clean_fixture(document);
        let restored = workspace.restore(&cleaned);
        assert!(
            restored.contains("Acme Consulting SARL"),
            "{document} did not restore its provider name:\n{restored}"
        );
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
    assert!(
        stderr.contains("scanned"),
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
