//! Command line behaviour, including the paths a user is most likely to get
//! wrong.

mod support;

use predicates::prelude::*;
use support::Workspace;

#[test]
fn clean_writes_a_sanitised_file_next_to_the_input() {
    let workspace = Workspace::new();
    let input = workspace.path().join("note.txt");
    std::fs::write(&input, "Call 06 12 34 56 78.\n").expect("writing the input");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .assert()
        .success();

    let output = std::fs::read_to_string(workspace.path().join("note.clean.md"))
        .expect("the sanitised file must exist");
    assert!(!output.contains("06 12 34 56 78"));
}

#[test]
fn clean_redacts_pii_in_the_output_filename() {
    let workspace = Workspace::new();
    let input = workspace.path().join("jean@example.com.txt");
    std::fs::write(&input, "Nothing sensitive in the body.\n").expect("writing the input");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .assert()
        .success();

    assert!(
        workspace.path().join("EMAIL_1.clean.md").is_file(),
        "the redacted name must be used"
    );
    assert!(
        !workspace.path().join("jean@example.com.clean.md").exists(),
        "the original PII filename must not survive"
    );
}

#[test]
fn clean_keeps_the_filename_when_redaction_is_disabled() {
    let workspace = Workspace::new();
    let input = workspace.path().join("jean@example.com.txt");
    std::fs::write(&input, "Nothing sensitive in the body.\n").expect("writing the input");
    let config = workspace.path().join("oboro.toml");
    std::fs::write(&config, "redact_filenames = false\n").expect("writing the configuration");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .arg("--config")
        .arg(&config)
        .assert()
        .success();

    assert!(
        workspace.path().join("jean@example.com.clean.md").is_file(),
        "the original name must be kept when redaction is off"
    );
}

#[test]
fn clean_keeps_tabular_extensions_in_the_output_name() {
    for (name, cleaned_name) in [
        ("data.csv", "data.clean.csv"),
        ("data.tsv", "data.clean.tsv"),
    ] {
        let workspace = Workspace::new();
        let input = workspace.path().join(name);
        std::fs::write(&input, "phone\n06 12 34 56 78\n").expect("writing the input");

        workspace
            .command()
            .arg("clean")
            .arg(&input)
            .assert()
            .success();

        let output = std::fs::read_to_string(workspace.path().join(cleaned_name))
            .expect("the sanitised tabular file must exist");
        assert!(!output.contains("06 12 34 56 78"));
        assert!(
            !workspace.path().join("data.clean.md").exists(),
            "a tabular input must not produce a markdown output"
        );
    }
}

#[test]
fn clean_writes_one_tsv_per_workbook_sheet() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(
        &book,
        &[
            (
                "Clients",
                &[&["name", "phone"], &["Jean", "06 12 34 56 78"]],
            ),
            ("Notes", &[&["topic"], &["Renewal"]]),
        ],
    );

    workspace
        .command()
        .arg("clean")
        .arg(&book)
        .assert()
        .success();

    let clients = std::fs::read_to_string(workspace.path().join("book.Clients.clean.tsv"))
        .expect("the first sheet must be written");
    assert!(!clients.contains("06 12 34 56 78"));
    assert!(
        !clients.contains("## "),
        "sheet headings must not appear in tabular output"
    );
    assert!(
        clients.contains("name\tphone"),
        "cells must stay tab-separated: {clients}"
    );

    assert!(
        workspace.path().join("book.Notes.clean.tsv").is_file(),
        "the second sheet must be written to its own file"
    );
    assert!(
        !workspace.path().join("book.clean.md").exists(),
        "a workbook must not produce a markdown output"
    );
}

#[test]
fn clean_numbers_sheets_whose_names_collide_after_sanitisation() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(&book, &[("a:b", &[&["first"]]), ("a*b", &[&["second"]])]);

    workspace
        .command()
        .arg("clean")
        .arg(&book)
        .assert()
        .success();

    assert!(workspace.path().join("book.a_b.clean.tsv").is_file());
    assert!(
        workspace.path().join("book.a_b_2.clean.tsv").is_file(),
        "a colliding fragment must be numbered apart, not overwritten"
    );
}

/// The input-level guard cannot see sheet names, so a sheet output clashing
/// with a plain input's output is caught against the destinations actually
/// written.
#[test]
fn clean_refuses_a_sheet_output_colliding_with_another_input() {
    let workspace = Workspace::new();
    let dir = workspace.path().join("docs");
    std::fs::create_dir_all(&dir).expect("creating the tree");
    std::fs::write(dir.join("book.Clients.tsv"), "phone\n06 12 34 56 78\n").expect("writing");
    support::write_xlsx(&dir.join("book.xlsx"), &[("Clients", &[&["value"]])]);

    workspace
        .command()
        .arg("clean")
        .arg(&dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("both be written to"));
}

/// A refused output must leave nothing in the vault: the collision is
/// detected before the document body is cleaned, so no placeholder is
/// allocated for values that were never written anywhere.
#[test]
fn a_refused_output_allocates_no_vault_entries() {
    let workspace = Workspace::new();
    let dir = workspace.path().join("docs");
    std::fs::create_dir_all(&dir).expect("creating the tree");
    std::fs::write(dir.join("book.Clients.tsv"), "note\nnothing sensitive\n").expect("writing");
    support::write_xlsx(
        &dir.join("book.xlsx"),
        &[("Clients", &[&["email"], &["colliding@refused.example"]])],
    );

    workspace
        .command()
        .arg("clean")
        .arg(&dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("both be written to"));

    workspace
        .command()
        .args(["map", "list", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("colliding@refused.example").not());
}

/// Two spellings of one destination must collide even when the paths differ
/// textually: the file the second write would replace is the same inode.
#[test]
fn clean_refuses_aliased_paths_naming_one_destination() {
    let workspace = Workspace::new();
    std::fs::write(workspace.path().join("note.txt"), "Mail a@example.com.\n").expect("writing");
    std::fs::write(workspace.path().join("note.md"), "Mail b@example.com.\n").expect("writing");

    // Relative and dot-prefixed spellings produce textually distinct
    // destinations (`note.clean.md` vs `./note.clean.md`) that are one file.
    workspace
        .command()
        .arg("clean")
        .arg("note.txt")
        .arg("./note.md")
        .assert()
        .failure()
        .stderr(predicate::str::contains("both be written to"));
}

/// Passing one workbook twice must be refused up front, as duplicates of any
/// other format are, rather than discovered sheet by sheet.
#[test]
fn clean_refuses_the_same_input_listed_twice() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(&book, &[("Clients", &[&["value"]])]);

    workspace
        .command()
        .arg("clean")
        .arg(&book)
        .arg(&book)
        .assert()
        .failure()
        .stderr(predicate::str::contains("listed twice"));
}

/// A workbook's outputs carry sheet fragments, so it cannot collide with a
/// plain input that shares only its stem.
#[test]
fn clean_accepts_a_workbook_beside_a_tabular_file_sharing_its_stem() {
    let workspace = Workspace::new();
    let dir = workspace.path().join("docs");
    std::fs::create_dir_all(&dir).expect("creating the tree");
    std::fs::write(dir.join("book.tsv"), "note\nnothing sensitive\n").expect("writing");
    support::write_xlsx(&dir.join("book.xlsx"), &[("Clients", &[&["value"]])]);

    workspace
        .command()
        .arg("clean")
        .arg(&dir)
        .assert()
        .success();

    assert!(dir.join("book.clean.tsv").is_file());
    assert!(dir.join("book.Clients.clean.tsv").is_file());
}

#[test]
fn clean_refuses_stdout_for_a_multi_sheet_workbook() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(&book, &[("One", &[&["a"]]), ("Two", &[&["b"]])]);

    workspace
        .command()
        .arg("clean")
        .arg(&book)
        .arg("--stdout")
        .assert()
        .failure()
        .stderr(predicate::str::contains("sheets"));
}

#[test]
fn clean_accepts_stdout_for_a_single_sheet_workbook() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(&book, &[("Only", &[&["phone"], &["06 12 34 56 78"]])]);

    workspace
        .command()
        .arg("clean")
        .arg(&book)
        .arg("--stdout")
        .assert()
        .success()
        .stdout(predicate::str::contains("06 12 34 56 78").not());
}

#[test]
fn clean_does_not_walk_its_own_tabular_outputs() {
    let workspace = Workspace::new();
    let dir = workspace.path().join("docs");
    std::fs::create_dir_all(&dir).expect("creating the tree");
    std::fs::write(dir.join("data.csv"), "phone\n06 12 34 56 78\n").expect("writing");
    std::fs::write(dir.join("done.clean.csv"), "already sanitised").expect("writing");
    std::fs::write(dir.join("done.clean.tsv"), "already sanitised").expect("writing");

    workspace
        .command()
        .arg("clean")
        .arg(&dir)
        .assert()
        .success();

    assert!(dir.join("data.clean.csv").is_file());
    assert!(
        !dir.join("done.clean.clean.csv").exists() && !dir.join("done.clean.clean.tsv").exists(),
        "existing outputs must not be cleaned again"
    );
}

#[test]
fn clean_honours_an_output_directory() {
    let workspace = Workspace::new();
    let input = workspace.path().join("note.txt");
    std::fs::write(&input, "Call 06 12 34 56 78.\n").expect("writing the input");
    let out_dir = workspace.path().join("sanitised");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .arg("--output")
        .arg(&out_dir)
        .assert()
        .success();

    assert!(out_dir.join("note.clean.md").is_file());
}

#[test]
fn clean_walks_a_directory_of_mixed_files() {
    let workspace = Workspace::new();
    let dir = workspace.path().join("docs");
    std::fs::create_dir_all(dir.join("sub")).expect("creating the tree");
    std::fs::write(dir.join("note.txt"), "Call 06 12 34 56 78.\n").expect("writing");
    std::fs::write(dir.join("archive.zip"), "binary").expect("writing");
    std::fs::write(dir.join("sub/deep.txt"), "Call 07 98 76 54 32.\n").expect("writing");
    let out_dir = workspace.path().join("sanitised");

    // Without --recursive the nested file is left untouched and the
    // unsupported archive is reported, not fatal.
    workspace
        .command()
        .arg("clean")
        .arg(&dir)
        .arg("--output")
        .arg(&out_dir)
        .assert()
        .success()
        .stderr(predicate::str::contains("1 unsupported file(s) skipped"));

    assert!(out_dir.join("note.clean.md").is_file());
    assert!(!out_dir.join("sub/deep.clean.md").exists());
}

#[test]
fn clean_recurses_and_mirrors_the_tree() {
    let workspace = Workspace::new();
    let dir = workspace.path().join("docs");
    std::fs::create_dir_all(dir.join("sub")).expect("creating the tree");
    std::fs::write(dir.join("note.txt"), "Call 06 12 34 56 78.\n").expect("writing");
    std::fs::write(dir.join("sub/deep.txt"), "Call 07 98 76 54 32.\n").expect("writing");
    let out_dir = workspace.path().join("sanitised");

    workspace
        .command()
        .arg("clean")
        .arg(&dir)
        .arg("--recursive")
        .arg("--output")
        .arg(&out_dir)
        .assert()
        .success();

    assert!(out_dir.join("note.clean.md").is_file());
    assert!(
        out_dir.join("sub/deep.clean.md").is_file(),
        "the input subdirectory must be mirrored under the output directory"
    );
}

#[test]
fn clean_refuses_inputs_that_share_an_output_name() {
    let workspace = Workspace::new();
    std::fs::write(workspace.path().join("contract.txt"), "one").expect("writing");
    std::fs::write(workspace.path().join("contract.docx"), "two").expect("writing");

    // Both would become contract.clean.md; refusing beats silently losing one.
    workspace
        .command()
        .arg("clean")
        .arg(workspace.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("both be written to"));
}

#[test]
fn clean_refuses_stdout_for_several_files() {
    let workspace = Workspace::new();
    let first = workspace.path().join("a.txt");
    let second = workspace.path().join("b.txt");
    std::fs::write(&first, "one").expect("writing");
    std::fs::write(&second, "two").expect("writing");

    workspace
        .command()
        .arg("clean")
        .arg(&first)
        .arg(&second)
        .arg("--stdout")
        .assert()
        .failure()
        .stderr(predicate::str::contains("single file"));
}

#[test]
fn clean_reports_an_unsupported_format() {
    let workspace = Workspace::new();
    let input = workspace.path().join("report.docx");
    std::fs::write(&input, "not really a docx").expect("writing");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .assert()
        .failure()
        .stderr(predicate::str::contains("docx"));
}

#[test]
fn clean_reports_a_missing_file() {
    let workspace = Workspace::new();
    workspace
        .command()
        .arg("clean")
        .arg(workspace.path().join("absent.txt"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("absent.txt"));
}

#[test]
fn clean_reports_a_missing_config_file() {
    let workspace = Workspace::new();
    let input = workspace.path().join("note.txt");
    std::fs::write(&input, "Call 06 12 34 56 78.\n").expect("writing the input");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .arg("--config")
        .arg(workspace.path().join("absent.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("absent.toml"));
}

#[test]
fn restore_reports_a_missing_file() {
    let workspace = Workspace::new();
    workspace
        .command()
        .arg("restore")
        .arg(workspace.path().join("absent.md"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("absent.md"));
}

/// A batch that fails on a later file must still report the failure and exit
/// non-zero, having written the outputs for the files that did succeed.
#[test]
fn clean_stops_and_reports_when_a_later_file_fails() {
    let workspace = Workspace::new();
    let good = workspace.path().join("good.txt");
    let bad = workspace.path().join("bad.docx");
    std::fs::write(&good, "Call 06 12 34 56 78.\n").expect("writing");
    std::fs::write(&bad, "not really a docx").expect("writing");

    workspace
        .command()
        .arg("clean")
        .arg(&good)
        .arg(&bad)
        .assert()
        .failure()
        .stderr(predicate::str::contains("docx"));

    assert!(
        workspace.path().join("good.clean.md").is_file(),
        "the file processed before the failure must still be written"
    );
}

#[test]
fn map_list_hides_values_unless_asked() {
    let workspace = Workspace::new();
    workspace.clean_fixture("contract.txt");

    workspace
        .command()
        .args(["map", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[[EMAIL_1]]"))
        .stdout(predicate::str::contains("@acme-consulting.example").not());

    workspace
        .command()
        .args(["map", "list", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("@acme-consulting.example"));
}

#[test]
fn map_purge_requires_confirmation() {
    let workspace = Workspace::new();
    workspace.clean_fixture("contract.txt");

    workspace
        .command()
        .args(["map", "purge"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));

    workspace
        .command()
        .args(["map", "purge", "--yes"])
        .assert()
        .success();

    workspace
        .command()
        .args(["map", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("empty"));
}

#[test]
fn restore_warns_about_placeholders_it_does_not_know() {
    let workspace = Workspace::new();
    let answer = workspace.path().join("answer.md");
    std::fs::write(&answer, "Ask [[PERSON_7]] about it.").expect("writing");

    workspace
        .command()
        .arg("restore")
        .arg(&answer)
        .assert()
        .success()
        .stderr(predicate::str::contains("unknown"));

    assert_eq!(
        std::fs::read_to_string(&answer).expect("reading back"),
        "Ask [[PERSON_7]] about it.",
        "unknown placeholders must be left untouched"
    );
}

#[test]
fn restore_rewrites_the_file_in_place_by_default() {
    let workspace = Workspace::new();
    let cleaned = workspace.clean_fixture("contract.txt");
    let answer = workspace.path().join("answer.md");
    std::fs::write(&answer, &cleaned).expect("writing");

    workspace
        .command()
        .arg("restore")
        .arg(&answer)
        .assert()
        .success();

    let restored = std::fs::read_to_string(&answer).expect("reading back");
    assert!(restored.contains("Jean Dupont"));
}

/// A document with nothing in it must not drag the user into a terminal
/// only to show an empty list.
#[test]
fn review_skips_a_document_with_nothing_to_redact() {
    let workspace = Workspace::new();
    let input = workspace.path().join("plain.txt");
    std::fs::write(&input, "Nothing sensitive at all in this line.\n").expect("writing");

    workspace
        .command()
        .arg("review")
        .arg(&input)
        .assert()
        .success()
        .stderr(predicate::str::contains("nothing detected"));

    assert!(
        !workspace.path().join("plain.clean.md").exists(),
        "skipping must not write an output file"
    );
}

#[test]
fn doctor_reports_the_vault_and_confirms_no_network_use() {
    let workspace = Workspace::new();
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("vault:"))
        .stdout(predicate::str::contains("network:"))
        .stdout(predicate::str::contains("model:"));
}

#[test]
fn separate_vaults_do_not_share_placeholders() {
    let first = Workspace::new();
    let second = Workspace::new();
    let cleaned = first.clean_fixture("contract.txt");

    let answer = second.path().join("answer.md");
    std::fs::write(&answer, &cleaned).expect("writing");
    let restored = second.restore(&cleaned);

    assert_eq!(
        restored, cleaned,
        "a second vault must not resolve another vault's placeholders"
    );
}
