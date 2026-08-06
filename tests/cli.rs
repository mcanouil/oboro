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
// Unix only: the guard is inode-based, and `identity_of` in `src/review/mod.rs`
// answers `None` off Unix, so on Windows the two spellings are not seen as one
// file. The guard being inert there is a real gap, tracked separately.
#[cfg(unix)]
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
fn clean_reads_standard_input_when_it_is_piped() {
    // Both spellings: an explicit `-`, and a bare invocation in a pipeline.
    for dash in [false, true] {
        let workspace = Workspace::new();
        let mut command = workspace.command();
        command.arg("clean");
        if dash {
            command.arg("-");
        }
        command
            .write_stdin("Call 06 12 34 56 78.\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("06 12 34 56 78").not())
            .stdout(predicate::str::contains("[[PHONE_1]]"));

        // Nothing may be written beside the invocation: a piped document has
        // no path, and a temporary file would be the leak this tool exists to
        // stop. The store itself is the only thing that may appear, and its
        // database carries sidecar files.
        let store = workspace.store_paths();
        let written: Vec<_> = std::fs::read_dir(workspace.path())
            .expect("reading the workspace")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                !store
                    .iter()
                    .any(|kept| path.to_string_lossy().starts_with(&*kept.to_string_lossy()))
            })
            .collect();
        assert!(written.is_empty(), "unexpected files written: {written:?}");
    }
}

#[test]
#[cfg(unix)]
fn clean_stops_quietly_when_the_reader_closes_the_pipe() {
    use std::io::Write as _;

    let workspace = Workspace::new();
    // Larger than a pipe buffer, so the write cannot land in the kernel's
    // buffer and succeed after the reader is already gone.
    let text = "Call 06 12 34 56 78.\n".repeat(10_000);

    let mut child = workspace
        .std_command("fr_FR.UTF-8")
        .arg("clean")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawning oboro");

    let mut stdin = child.stdin.take().expect("the child's standard input");
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(text.as_bytes());
    });
    // The reader leaves before reading anything, as `| head -n 1` does.
    drop(child.stdout.take());

    let output = child.wait_with_output().expect("waiting for oboro");
    writer.join().expect("the writing thread");
    assert!(
        output.status.success(),
        "a reader closing the pipe is a normal way to stop, not a crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The write end of a pipe whose reader has already exited.
///
/// These reports are small enough to fit in a pipe buffer, so dropping the
/// reader after the child starts would race it. The read end is closed before
/// `oboro` runs instead, so its very first write fails and the outcome is the
/// same every run.
#[cfg(unix)]
fn closed_pipe() -> std::process::Stdio {
    let mut reader = std::process::Command::new("true")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawning the reader");
    let pipe = reader.stdin.take().expect("the reader's standard input");
    reader.wait().expect("waiting for the reader");
    std::process::Stdio::from(pipe)
}

#[test]
#[cfg(unix)]
fn doctor_stops_quietly_when_the_reader_closes_the_pipe() {
    let workspace = Workspace::new();

    let output = workspace
        .std_command("fr_FR.UTF-8")
        .arg("doctor")
        .stdout(closed_pipe())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawning oboro")
        .wait_with_output()
        .expect("waiting for oboro");

    assert!(
        output.status.success(),
        "a reader closing the pipe is a normal way to stop, not a crash: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs `oboro` with the given arguments and nothing left reading its standard
/// error, as `2>&1 | head -n 1` leaves it.
#[cfg(unix)]
fn run_with_closed_error_pipe(workspace: &Workspace, args: &[&str]) -> std::process::Output {
    workspace
        .std_command("fr_FR.UTF-8")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(closed_pipe())
        .spawn()
        .expect("spawning oboro")
        .wait_with_output()
        .expect("waiting for oboro")
}

/// `oboro map list 2>&1 | head -n 1`: the progress and summary lines go to
/// standard error, and a reader leaving is as normal there as on standard
/// output.
#[test]
#[cfg(unix)]
fn map_list_stops_quietly_when_the_reader_closes_the_error_pipe() {
    let workspace = Workspace::new();
    let output = run_with_closed_error_pipe(&workspace, &["map", "list"]);

    assert!(
        output.status.success(),
        "a reader closing the error pipe is a normal way to stop, not a crash"
    );
}

/// A command that fails reports it on standard error, so a closed error pipe
/// must not turn the exit code the caller reads into a panic's 101.
#[test]
#[cfg(unix)]
fn an_error_keeps_its_exit_code_when_the_reader_closes_the_error_pipe() {
    let workspace = Workspace::new();
    let output = run_with_closed_error_pipe(&workspace, &["restore", "missing.txt"]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a failure must still exit 1, whatever standard error does"
    );
}

#[test]
fn clean_refuses_a_dash_alongside_a_file() {
    let workspace = Workspace::new();
    let input = workspace.path().join("note.txt");
    std::fs::write(&input, "Nothing here.\n").expect("writing the input");

    workspace
        .command()
        .arg("clean")
        .arg("-")
        .arg(&input)
        .write_stdin("Call 06 12 34 56 78.\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("standard input"));
}

#[test]
fn clean_refuses_an_output_directory_for_standard_input() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("clean")
        .arg("-")
        .arg("--output")
        .arg(workspace.path().join("out"))
        .write_stdin("Call 06 12 34 56 78.\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output"));
}

#[test]
fn clean_refuses_binary_standard_input() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("clean")
        .write_stdin(vec![0x00, 0xff, 0xfe, b'a'])
        .assert()
        .failure()
        .stderr(predicate::str::contains("UTF-8"));
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

#[test]
fn restore_reads_standard_input_when_it_is_piped() {
    // Both spellings: an explicit `-`, and a bare invocation in a pipeline.
    for dash in [false, true] {
        let workspace = Workspace::new();
        let cleaned = workspace.clean_fixture("contract.txt");
        let mut command = workspace.command();
        command.arg("restore");
        if dash {
            command.arg("-");
        }
        command
            .write_stdin(cleaned)
            .assert()
            .success()
            .stdout(predicate::str::contains("Jean Dupont"));

        // Piped text has no path to rewrite in place, so nothing may be
        // written beside the invocation. Only the store may appear, and its
        // database carries sidecar files.
        let store = workspace.store_paths();
        let written: Vec<_> = std::fs::read_dir(workspace.path())
            .expect("reading the workspace")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                !store
                    .iter()
                    .any(|kept| path.to_string_lossy().starts_with(&*kept.to_string_lossy()))
            })
            .collect();
        assert!(written.is_empty(), "unexpected files written: {written:?}");
    }
}

/// The shape an agent hook takes: text in memory cleaned on the way out and
/// restored on the way back, with nothing written to disk at either end.
#[test]
fn text_round_trips_through_both_pipes() {
    let workspace = Workspace::new();
    let original = "Call Jean Dupont on 06 12 34 56 78.\n";

    let cleaned = workspace.clean_piped(original);
    assert!(
        !cleaned.contains("06 12 34 56 78"),
        "the value survived the piped clean:\n{cleaned}"
    );
    assert_eq!(
        workspace.restore_piped(&cleaned),
        original,
        "a piped round trip must reproduce the original text"
    );
}

/// Nothing to clean is not a failure: a hook wrapping a tool that produced no
/// output must not turn that into an error the user has to read.
#[test]
fn empty_standard_input_produces_empty_output() {
    for command in ["clean", "restore"] {
        let workspace = Workspace::new();
        workspace
            .command()
            .arg(command)
            .write_stdin("")
            .assert()
            .success()
            .stdout(predicate::str::is_empty());
    }
}

#[test]
fn restore_refuses_binary_standard_input() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("restore")
        .write_stdin(vec![0x00, 0xff, 0xfe, b'a'])
        .assert()
        .failure()
        .stderr(predicate::str::contains("UTF-8"));
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

/// The hooks are named in the user's own settings rather than written there by
/// Oboro, so `doctor` is the only way to tell protection apart from the belief
/// in it.
#[test]
fn doctor_reports_whether_the_hooks_are_installed() {
    let workspace = Workspace::new();

    // Nothing installed: both halves must be named, since a user with neither
    // has to be told so.
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("PostToolUse not installed"))
        .stdout(predicate::str::contains("PreToolUse  not installed"));

    let settings = workspace.path().join(".claude");
    std::fs::create_dir_all(&settings).expect("creating the settings directory");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"hooks":{"PostToolUse":[{"matcher":"Read|Grep",
             "hooks":[{"type":"command","command":"oboro hook post-tool-use"}]}]}}"#,
    )
    .expect("writing the settings");

    // Half installed, which is the state worth reporting: the model is shown
    // placeholders and nothing puts them back.
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("settings.json"))
        .stdout(predicate::str::contains("Read|Grep"))
        .stdout(predicate::str::contains("PreToolUse  not installed"));
}

/// A hook in a project's shared settings runs like any other and is reported
/// like any other, but `oboro uninstall` never writes that file and so never
/// takes it out again. Saying so here is what stops a full uninstall being read
/// as having removed it.
#[test]
fn doctor_marks_a_hook_an_uninstall_will_leave_behind() {
    let workspace = Workspace::new();
    let settings = workspace.path().join(".claude");
    std::fs::create_dir_all(&settings).expect("creating the settings directory");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"hooks":{"PostToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"oboro hook post-tool-use"}]}]}}"#,
    )
    .expect("planting a hook by hand");

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "`oboro uninstall` leaves this file",
        ));

    // The file a project install writes carries no such marker: an uninstall
    // does take that one out, and a marker on every hook would say nothing.
    std::fs::remove_file(settings.join("settings.json")).expect("removing the planted hook");
    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--project")
        .assert()
        .success();
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("settings.local.json"))
        .stdout(predicate::str::contains("leaves this file").not());
}

/// The plugin brings its own hooks, and they live in the agent's plugin cache
/// rather than in any settings file. Reporting them as missing would send a
/// protected user to `oboro hook install` and leave them running two copies.
#[test]
fn doctor_reports_the_hooks_the_plugin_carries() {
    let workspace = Workspace::new();

    let settings = workspace.path().join(".claude");
    std::fs::create_dir_all(&settings).expect("creating the settings directory");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"enabledPlugins":{"oboro@oboro":true,"something-else@elsewhere":true}}"#,
    )
    .expect("writing the settings");

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("plugin      "))
        .stdout(predicate::str::contains("oboro@oboro, enabled"))
        .stdout(predicate::str::contains("something-else").not())
        .stdout(predicate::str::contains("PostToolUse not in your settings"))
        .stdout(predicate::str::contains("run `oboro hook install`").not())
        // The plugin carries a skill too, so reporting both scopes as simply
        // missing would be the same false report on the other half.
        .stdout(predicate::str::contains(
            "not installed here; the plugin carries its own",
        ));
}

/// `hook install` is the moment a second copy of the hooks is created, and both
/// copies then run on every matching tool call. Saying so afterwards in
/// `doctor` is too late to stop it.
#[test]
fn hook_install_says_so_when_a_plugin_already_carries_the_hooks() {
    let workspace = Workspace::new();

    let settings = workspace.path().join(".claude");
    std::fs::create_dir_all(&settings).expect("creating the settings directory");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"enabledPlugins":{"oboro@oboro":true}}"#,
    )
    .expect("writing the settings");

    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--project")
        .assert()
        .success()
        .stderr(predicate::str::contains("oboro@oboro"))
        .stderr(predicate::str::contains("twice"));
}

/// A plugin the user turned off carries nothing, so saying otherwise would be
/// the same lie in the other direction.
#[test]
fn doctor_ignores_a_plugin_that_is_installed_but_disabled() {
    let workspace = Workspace::new();

    let settings = workspace.path().join(".claude");
    std::fs::create_dir_all(&settings).expect("creating the settings directory");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"enabledPlugins":{"oboro@oboro":false}}"#,
    )
    .expect("writing the settings");

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("plugin      ").not())
        .stdout(predicate::str::contains("PostToolUse not installed"));
}

#[test]
fn doctor_says_where_the_phone_regions_came_from() {
    let workspace = Workspace::new();
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("regions:"))
        .stdout(predicate::str::contains("FR (from $LC_ALL)"));

    std::fs::write(workspace.path().join("oboro.toml"), "regions = [\"GB\"]\n")
        .expect("writing the configuration");
    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("GB (from oboro.toml)"));
}

/// A configuration that will not load is exactly when `doctor` is run, so the
/// lines naming the vault, the key and the offending file are still written
/// before the error is reported.
#[test]
fn doctor_names_the_files_it_read_before_refusing_a_bad_configuration() {
    let workspace = Workspace::new();
    std::fs::write(workspace.path().join("oboro.toml"), "regions = [\"ZZ\"]\n")
        .expect("writing the configuration");

    workspace
        .command()
        .arg("doctor")
        .assert()
        .failure()
        .stdout(predicate::str::contains("vault:"))
        .stdout(predicate::str::contains("oboro.toml"))
        .stderr(predicate::str::contains("not a two-letter region code"));
}

/// No locale, no configuration, and structured identifiers are still found:
/// nothing about detection requires a language or a region to be declared.
#[test]
fn a_document_is_cleaned_with_no_locale_and_no_configuration() {
    let workspace = Workspace::new();
    let input = workspace.path().join("mixed.txt");
    std::fs::write(
        &input,
        "Ring +33 1 42 68 53 00 or mail sales@example.com.\n\
         Deliver to 10 Downing Street, SW1A 1AA.\n\
         Hauptstraße 5 is the other address.\n",
    )
    .expect("writing the input");

    workspace
        .command_in_locale("C")
        .arg("clean")
        .arg(&input)
        .assert()
        .success();

    let output = std::fs::read_to_string(workspace.path().join("mixed.clean.md"))
        .expect("the sanitised file must exist");
    for secret in [
        "+33 1 42 68 53 00",
        "sales@example.com",
        "10 Downing Street",
        "SW1A 1AA",
        "Hauptstraße 5",
    ] {
        assert!(
            !output.contains(secret),
            "{secret} survived with no locale set: {output}"
        );
    }
}

/// A region is a hint that widens what is read, never a requirement: with no
/// locale the international number is still caught and the national one is not.
#[test]
fn a_region_hint_only_widens_what_is_read() {
    let workspace = Workspace::new();
    let input = workspace.path().join("calls.txt");
    let text = "Ring +33 1 42 68 53 00 or 06 12 34 56 78.\n";
    std::fs::write(&input, text).expect("writing the input");

    workspace
        .command_in_locale("C")
        .arg("clean")
        .arg(&input)
        .arg("--stdout")
        .assert()
        .success()
        .stdout(predicate::str::contains("+33 1 42 68 53 00").not())
        .stdout(predicate::str::contains("06 12 34 56 78"));

    std::fs::write(workspace.path().join("oboro.toml"), "regions = [\"FR\"]\n")
        .expect("writing the configuration");
    workspace
        .command_in_locale("C")
        .arg("clean")
        .arg(&input)
        .arg("--stdout")
        .assert()
        .success()
        .stdout(predicate::str::contains("06 12 34 56 78").not());
}

#[test]
fn an_unknown_region_is_refused_with_the_code_named() {
    let workspace = Workspace::new();
    let input = workspace.path().join("note.txt");
    std::fs::write(&input, "Nothing here.\n").expect("writing the input");
    std::fs::write(workspace.path().join("oboro.toml"), "regions = [\"XX\"]\n")
        .expect("writing the configuration");

    workspace
        .command()
        .arg("clean")
        .arg(&input)
        .assert()
        .failure()
        .stderr(predicate::str::contains("XX"));
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

/// The skill an agent reads is the skill the binary carries, so `show` and the
/// installed file have to be the same text: an install that quietly wrote
/// something else would teach the agent the wrong thing with no way to tell.
#[test]
fn skill_install_writes_the_text_skill_show_prints() {
    let workspace = Workspace::new();
    let shown = workspace
        .command()
        .arg("skill")
        .arg("show")
        .output()
        .expect("running oboro skill show");
    assert!(shown.status.success());

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--project")
        .assert()
        .success();

    let installed = std::fs::read_to_string(workspace.path().join(".claude/skills/oboro/SKILL.md"))
        .expect("the skill must have been written");
    assert_eq!(installed.as_bytes(), shown.stdout.as_slice());
}

#[test]
fn skill_install_dry_run_names_the_path_and_writes_nothing() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--project")
        .arg("--dry-run")
        .assert()
        .success()
        // Built from components rather than written out, since the separator
        // the path is printed with is the platform's.
        .stderr(predicate::str::contains(
            std::path::Path::new(".claude")
                .join("skills")
                .join("oboro")
                .join("SKILL.md")
                .display()
                .to_string(),
        ))
        .stderr(predicate::str::contains("nothing was written"));

    assert!(
        !workspace.path().join(".claude").exists(),
        "a dry run must not create the directory either"
    );
}

/// The two halves are one decision: a skill describing placeholders no hook
/// produces explains nothing, and hooks without the skill leave the agent
/// guessing at what it is being shown.
#[test]
fn skill_install_with_hooks_installs_both_halves_into_one_scope() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--project")
        .arg("--with-hooks")
        .assert()
        .success()
        .stderr(predicate::str::contains("SKILL.md"))
        .stderr(predicate::str::contains("PostToolUse"))
        .stderr(predicate::str::contains("PreToolUse"));

    assert!(
        workspace
            .path()
            .join(".claude/skills/oboro/SKILL.md")
            .exists()
    );
    let settings = std::fs::read_to_string(workspace.path().join(".claude/settings.local.json"))
        .expect("reading the settings");
    assert!(settings.contains("oboro hook post-tool-use"));
    assert!(settings.contains("oboro hook pre-tool-use"));

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("(current)"))
        .stdout(predicate::str::contains("settings.local.json").count(2));
}

/// A dry run has to cover both halves or it would answer a question it was not
/// asked: what one of the two would do.
#[test]
fn skill_install_with_hooks_dry_run_shows_both_and_writes_neither() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--project")
        .arg("--with-hooks")
        .arg("--dry-run")
        .assert()
        .success()
        .stderr(predicate::str::contains("SKILL.md"))
        .stderr(predicate::str::contains("nothing was written"))
        .stdout(predicate::str::contains("oboro hook post-tool-use"));

    assert!(
        !workspace.path().join(".claude").exists(),
        "a dry run must not create the directory either"
    );
}

/// Both plans are made before either is carried out, so a scope that refuses
/// one half installs neither. Half an install is the state the pair exists to
/// avoid: the skill would describe hooks that are not there.
#[cfg(unix)]
#[test]
fn skill_install_with_hooks_writes_nothing_when_the_settings_are_a_symbolic_link() {
    let workspace = Workspace::new();
    let elsewhere = workspace.path().join("elsewhere.json");
    std::fs::write(&elsewhere, "{}").expect("writing the link target");
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("creating .claude");
    std::os::unix::fs::symlink(
        &elsewhere,
        workspace.path().join(".claude/settings.local.json"),
    )
    .expect("linking");

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--project")
        .arg("--with-hooks")
        .assert()
        .failure()
        .stderr(predicate::str::contains("symbolic link"));

    assert!(
        !workspace
            .path()
            .join(".claude/skills/oboro/SKILL.md")
            .exists(),
        "the skill must not be written when the hooks cannot be"
    );
    assert_eq!(
        std::fs::read_to_string(&elsewhere).expect("reading the link target"),
        "{}",
        "nothing is written through the link"
    );
}

/// Choosing the scope is interactive, and a script has no one to ask. Guessing
/// would be worse than failing: the wrong scope installs a skill the agent
/// never reads, and says nothing.
#[test]
fn skill_install_without_a_scope_and_without_a_terminal_names_both_flags() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .write_stdin(String::new())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--project"))
        .stderr(predicate::str::contains("--user"));
}

// Unix only: the harness redirects the home directory with `HOME`, which
// `dirs::home_dir` honours there. On Windows it asks the shell for the profile
// folder and ignores the environment, so this would write into the runner's own
// profile and then assert against a directory the command never touched.
// Covering the user scope on Windows needs a seam in the product.
#[cfg(unix)]
#[test]
fn skill_install_for_every_project_writes_under_the_home_directory() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--user")
        .assert()
        .success();

    assert!(
        workspace
            .home()
            .join(".claude/skills/oboro/SKILL.md")
            .exists(),
        "the skill must be written under the home directory"
    );
    assert!(
        !workspace.path().join(".claude").exists(),
        "and not into the project as well"
    );
}

/// `doctor` is how a user checks rather than assumes, and the skill is one more
/// thing they cannot see from the outside.
#[test]
fn doctor_reports_the_skill_once_it_is_installed() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("skill").and(predicate::str::contains("not installed")));

    workspace
        .command()
        .arg("skill")
        .arg("install")
        .arg("--project")
        .assert()
        .success();

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("(current)"));
}

#[test]
fn hook_install_writes_the_local_settings_and_doctor_finds_them() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--project")
        .assert()
        .success()
        .stderr(predicate::str::contains("PostToolUse"))
        .stderr(predicate::str::contains("PreToolUse"));

    assert!(
        workspace
            .path()
            .join(".claude/settings.local.json")
            .exists()
    );
    assert!(
        !workspace.path().join(".claude/settings.json").exists(),
        "the shared settings are not this command's to write"
    );

    workspace
        .command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("settings.local.json").count(2));
}

// Unix only, for the reason given above the skill's equivalent: the home
// directory the harness sets is invisible to `dirs::home_dir` on Windows.
#[cfg(unix)]
#[test]
fn hook_install_for_every_project_writes_under_the_home_directory() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--user")
        .assert()
        .success();

    assert!(workspace.home().join(".claude/settings.json").exists());
    assert!(!workspace.path().join(".claude").exists());
}

/// The whole point of the merge is that it can be run on a file someone else
/// wrote, so the dry run has to show that file as it would end up.
#[test]
fn hook_install_dry_run_prints_the_settings_and_writes_nothing() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--project")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("oboro hook post-tool-use"))
        .stdout(predicate::str::contains("oboro hook pre-tool-use"))
        .stderr(predicate::str::contains("nothing was written"));

    assert!(
        !workspace.path().join(".claude").exists(),
        "a dry run must not create the directory either"
    );
}

#[test]
fn hook_install_without_a_scope_and_without_a_terminal_names_both_flags() {
    let workspace = Workspace::new();

    workspace
        .command()
        .arg("hook")
        .arg("install")
        .write_stdin(String::new())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--project"))
        .stderr(predicate::str::contains("--user"));
}

/// A repository shipping its own `.claude` could otherwise point the settings
/// at a file outside it and have the installer write there.
#[test]
#[cfg(unix)]
fn hook_install_refuses_a_symlinked_settings_file() {
    let workspace = Workspace::new();
    let elsewhere = workspace.path().join("elsewhere.json");
    std::fs::write(&elsewhere, "{}").expect("writing the link target");
    std::fs::create_dir_all(workspace.path().join(".claude")).expect("creating .claude");
    std::os::unix::fs::symlink(
        &elsewhere,
        workspace.path().join(".claude/settings.local.json"),
    )
    .expect("linking");

    workspace
        .command()
        .arg("hook")
        .arg("install")
        .arg("--project")
        .assert()
        .failure()
        .stderr(predicate::str::contains("symbolic link"));

    assert_eq!(
        std::fs::read_to_string(&elsewhere).expect("reading the link target"),
        "{}",
        "the link target is untouched"
    );
}

/// The whole design of `completions` is the split between the two streams: the
/// redirect has to capture a script the shell can read, and the instructions
/// have to reach the person who ran the command.
#[test]
fn completions_writes_the_script_to_stdout_and_the_destination_to_stderr() {
    let workspace = Workspace::new();
    let output = workspace
        .command()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("running oboro completions");

    assert!(output.status.success());
    let script = String::from_utf8(output.stdout).expect("the script must be UTF-8");
    let hint = String::from_utf8(output.stderr).expect("the hint must be UTF-8");

    assert!(
        script.starts_with("#compdef oboro"),
        "the redirect must capture a script and nothing before it: {:?}",
        script.lines().next()
    );
    assert!(
        !script.contains("m.canouil.dev"),
        "no part of the hint may reach the file"
    );
    assert!(
        hint.contains("_oboro"),
        "the hint must name the destination"
    );
    assert!(
        hint.contains("autoload -Uz compinit && compinit"),
        "the hint must print the command that makes zsh read it"
    );
}

/// A build directory or a renamed copy still has to complete the installed
/// name, so the script is generated under the name the command carries.
#[test]
fn completions_generates_under_the_installed_name() {
    let workspace = Workspace::new();
    workspace
        .command()
        .arg("completions")
        .arg("bash")
        .assert()
        .success()
        .stdout(predicate::str::contains("_oboro()"))
        .stdout(predicate::str::contains("target/debug").not());
}

/// A completion script is a copy of the command surface from when it was
/// generated, and nothing else in the tool would say it has fallen behind.
// Unix only, for the reason given above the skill's equivalent: the home
// directory the harness sets is invisible to `dirs::home_dir` on Windows, so
// `doctor` would walk the runner's own profile rather than the directory this
// writes the script into. The staleness comparison itself is covered on every
// platform by the unit tests in `src/completions.rs`, which resolve an
// `Environment` against a temporary directory.
#[cfg(unix)]
#[test]
fn doctor_reports_a_completion_script_as_current_then_stale() {
    let workspace = Workspace::new();
    let directory = workspace.home().join(".zfunc");
    std::fs::create_dir_all(&directory).expect("creating the completion directory");
    let script = directory.join("_oboro");

    // Nothing installed anywhere: said rather than passed over, as the hooks
    // and the skill are.
    workspace
        .completions_command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("completion  not installed"));

    let generated = workspace
        .completions_command()
        .arg("completions")
        .arg("zsh")
        .output()
        .expect("running oboro completions");
    std::fs::write(&script, &generated.stdout).expect("writing the completion script");

    workspace
        .completions_command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "completion  {} (current)",
            script.display()
        )));

    std::fs::write(&script, b"# an older release wrote this\n").expect("editing the script");

    workspace
        .completions_command()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("(stale)"))
        // The command rather than the path: `--install` finds the file itself.
        .stdout(predicate::str::contains("oboro completions zsh --install"));
}

/// The installer checks the same places in shell that `conventional_paths`
/// checks in Rust, so the list is written twice and can drift. The home
/// directory and the environment overrides are what a test cannot compare, so
/// it compares the part of each path that neither of them changes.
#[test]
fn the_installer_checks_every_conventional_directory() {
    // The installer writes the binary's name through a variable, which is the
    // right thing for a shell script and unreadable to a plain search, so it is
    // expanded here rather than spelled out twice there.
    let installer = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/install.sh"),
    )
    .expect("reading docs/install.sh")
    .replace("${BINARY_NAME}", "oboro");

    let home = std::path::Path::new("/home/someone");
    let environment = oboro::completions::Environment {
        name: "oboro".to_owned(),
        home: home.to_owned(),
        data: home.join(".local/share"),
        config: home.join(".config"),
        oh_my_zsh: home.join(".oh-my-zsh/custom"),
        homebrew: Some(std::path::PathBuf::from("/opt/homebrew")),
    };

    for location in oboro::completions::conventional_paths(&environment) {
        assert!(
            installer.contains(location.convention),
            "docs/install.sh does not look in {}, which {} reads",
            location.convention,
            location.shell
        );
    }
}

/// The flags belong to the commands that open a vault, so they are accepted
/// after the command that uses them.
#[test]
fn the_store_flags_are_accepted_by_the_commands_that_open_a_vault() {
    let workspace = Workspace::new();
    let vault = workspace.path().join("elsewhere.db");

    workspace
        .command()
        .arg("doctor")
        .arg("--vault")
        .arg(&vault)
        .arg("--key")
        .arg(workspace.path().join("elsewhere.key"))
        .assert()
        .success()
        .stdout(predicate::str::contains(vault.display().to_string()));
}

/// Declared on the group rather than on each leaf, so `map list` and
/// `map purge` are covered without the flags being repeated on both, and either
/// position parses.
#[test]
fn the_store_flags_reach_a_subcommand_of_a_command_that_takes_them() {
    let workspace = Workspace::new();
    let vault = workspace.path().join("elsewhere.db");

    for arguments in [
        vec!["map", "--vault", vault.to_str().expect("a path"), "list"],
        vec!["map", "list", "--vault", vault.to_str().expect("a path")],
    ] {
        workspace
            .command()
            .args(&arguments)
            .assert()
            .success()
            .stderr(predicate::str::contains("the vault is empty"));
    }
}

/// A command that never opens a vault refuses them rather than accepting and
/// ignoring them, which is what listing them in its help amounted to.
#[test]
fn a_command_that_never_opens_a_vault_refuses_the_store_flags() {
    let workspace = Workspace::new();

    for arguments in [
        vec!["skill", "show"],
        vec!["hook", "install", "--dry-run", "--project"],
    ] {
        workspace
            .command()
            .args(&arguments)
            .arg("--vault")
            .arg(workspace.path().join("elsewhere.db"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument '--vault'"));
    }
}

/// The help under a command is the same list the completion script offers, so a
/// flag named there that the command ignores is wrong in two places at once.
#[test]
fn the_help_names_the_store_flags_only_where_they_work() {
    let workspace = Workspace::new();

    let takes_them = workspace
        .command()
        .arg("clean")
        .arg("--help")
        .output()
        .expect("running oboro clean --help");
    assert!(
        String::from_utf8_lossy(&takes_them.stdout).contains("--vault"),
        "clean opens a vault and must say so"
    );

    let does_not = workspace
        .command()
        .arg("skill")
        .arg("show")
        .arg("--help")
        .output()
        .expect("running oboro skill show --help");
    assert!(
        !String::from_utf8_lossy(&does_not.stdout).contains("--vault"),
        "skill show never opens a vault and must not offer to"
    );
}
