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

#[test]
fn no_planted_value_survives_cleaning() {
    let workspace = Workspace::new();
    let cleaned = workspace.clean_fixture("contract.txt");

    let leaked: Vec<&str> = PLANTED
        .iter()
        .copied()
        .filter(|planted| cleaned.contains(planted))
        .collect();

    assert!(
        leaked.is_empty(),
        "sanitised output leaked {} value(s): {leaked:#?}\n\n--- output ---\n{cleaned}",
        leaked.len()
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

#[test]
fn allowlisted_values_are_preserved() {
    let workspace = Workspace::new();
    let cleaned = workspace.clean_fixture("contract.txt");
    assert!(
        cleaned.contains("Lille"),
        "an allowlisted value was redacted:\n{cleaned}"
    );
}
