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

#[test]
fn doctor_reports_the_vault_and_confirms_no_network_use() {
    let workspace = Workspace::new();
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("vault:"))
        .stdout(predicate::str::contains("never contacted"));
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
