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
