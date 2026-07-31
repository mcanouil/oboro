//! The MCP server, which is the path where an agent reaches Oboro in a client
//! that has no hook system.
//!
//! The assertions that matter most are the ones about what does *not* come
//! back: there is no `restore` tool, no way to reveal the mapping, and no
//! fragment of a document in an error.

mod support;

use support::Workspace;

/// Runs a session, writing every line in `messages` to the server and
/// returning the replies it wrote back.
/// Runs a session that may read anywhere.
///
/// `--unconfined` rather than nothing: naming roots is the default and reading
/// anywhere has to be asked for, so a bare `oboro mcp` refuses to start. The
/// tests that predate the roots work are about something else, and passing the
/// flag keeps them testing what they were written to test.
fn session(workspace: &Workspace, messages: &[&str]) -> Vec<serde_json::Value> {
    session_with(workspace, &["--unconfined"], messages)
}

/// Runs a session with extra arguments after `mcp`.
///
/// `--config` is a flag on the `mcp` subcommand itself, matching `clean`'s
/// `--config`, rather than a global one, so it must follow `mcp` on the
/// command line rather than precede it.
fn session_with(workspace: &Workspace, args: &[&str], messages: &[&str]) -> Vec<serde_json::Value> {
    let mut command = workspace.command();
    command.arg("mcp");
    for arg in args {
        command.arg(arg);
    }
    let output = command
        .write_stdin(format!("{}\n", messages.join("\n")))
        .output()
        .expect("running oboro mcp");
    assert!(
        output.status.success(),
        "the server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("output must be UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON message"))
        .collect()
}

/// A `tools/call` message for `name` with `arguments`.
fn call(name: &str, arguments: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
    )
}

/// The text of every content block in a `tools/call` result, joined.
fn text_of(reply: &serde_json::Value) -> String {
    reply["result"]["content"]
        .as_array()
        .expect("a content array")
        .iter()
        .map(|block| block["text"].as_str().expect("a text block"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_server_completes_a_handshake_and_lists_its_tools() {
    let workspace = Workspace::new();
    let replies = session(
        &workspace,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ],
    );

    // Two replies, not three: the notification must go unanswered.
    assert_eq!(replies.len(), 2, "a notification was answered: {replies:?}");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .expect("an array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, ["clean", "map_list"]);
}

#[test]
fn nothing_but_protocol_messages_reaches_standard_output() {
    let workspace = Workspace::new();
    let output = workspace
        .command()
        .arg("mcp")
        .write_stdin("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_owned())
        .output()
        .expect("running oboro mcp");
    for line in String::from_utf8(output.stdout)
        .expect("output must be UTF-8")
        .lines()
    {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|_| panic!("stdout must carry only MCP messages, found: {line}"));
    }
}

#[test]
fn map_list_reports_placeholders_without_values_or_timestamps() {
    let workspace = Workspace::new();
    // Seeded over the command line rather than over the protocol: the `clean`
    // tool does not run yet, and this asserts what `map_list` returns whatever
    // filled the vault.
    workspace.clean_piped("Call Marie on 06 12 34 56 78.");

    let replies = session(&workspace, &[&call("map_list", "{}")]);

    // `oboro map list` prints a created_at column beside each placeholder; this
    // deliberately prints the placeholders and nothing else, so the listing is
    // compared whole rather than searched for anything a timestamp looks like.
    assert_eq!(text_of(&replies[0]), "[[PHONE_1]]");
}

#[test]
fn an_empty_vault_says_so_rather_than_returning_nothing() {
    let workspace = Workspace::new();
    let replies = session(&workspace, &[&call("map_list", "{}")]);
    assert_eq!(replies[0]["result"]["isError"], false);
    assert!(
        !text_of(&replies[0]).is_empty(),
        "an empty vault must still say something: {}",
        replies[0]
    );
}

#[test]
fn an_unknown_tool_is_a_tool_error_rather_than_a_protocol_error() {
    let workspace = Workspace::new();
    let replies = session(&workspace, &[&call("restore", r#"{"path":"x"}"#)]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(
        replies[0].get("error").is_none(),
        "a missing tool is not a protocol fault: {}",
        replies[0]
    );
}

#[test]
fn tools_call_without_params_is_an_invalid_params_error() {
    let workspace = Workspace::new();
    let replies = session(
        &workspace,
        &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#],
    );
    assert_eq!(replies[0]["error"]["code"], -32602);
}

#[test]
fn clean_replaces_values_with_placeholders() {
    let workspace = Workspace::new();
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "Call Marie on 06 12 34 56 78.").expect("writing the note");

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, file.display()),
        )],
    );

    let cleaned = text_of(&replies[0]);
    assert!(
        !cleaned.contains("06 12 34 56 78"),
        "the value reached the model:\n{cleaned}"
    );
    assert!(
        cleaned.contains("[[PHONE_1]]"),
        "the placeholder is missing:\n{cleaned}"
    );
    assert_eq!(replies[0]["result"]["isError"], false);
}

#[test]
fn each_sheet_of_a_workbook_becomes_its_own_block() {
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(
        &book,
        &[
            ("First", &[&["one"], &["06 12 34 56 78"]]),
            ("Second", &[&["two"]]),
        ],
    );

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, book.display()),
        )],
    );

    let blocks = replies[0]["result"]["content"]
        .as_array()
        .expect("a content array");
    assert_eq!(blocks.len(), 2, "one block per sheet: {blocks:?}");
    let first = blocks[0]["text"].as_str().expect("text");
    assert!(first.contains("## First"));
    assert!(
        blocks[1]["text"]
            .as_str()
            .expect("text")
            .contains("## Second")
    );
    // The heading is not the only thing cleaned: the cells under it are the
    // reason the tool exists.
    assert!(
        !first.contains("06 12 34 56 78"),
        "a value in the body reached the model:\n{first}"
    );
    assert!(
        first.contains("[[PHONE_1]]"),
        "the body was not cleaned:\n{first}"
    );
}

#[test]
fn a_missing_path_is_a_tool_error_and_the_loop_survives_it() {
    let workspace = Workspace::new();
    let missing = workspace.path().join("nowhere.txt");

    let replies = session(
        &workspace,
        &[
            &call("clean", &format!(r#"{{"path":"{}"}}"#, missing.display())),
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ],
    );

    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(
        replies[1]["result"].is_object(),
        "a tool failure must not end the loop: {:?}",
        replies[1]
    );
}

/// The first call names a file that does not exist, so `clean` returns before
/// it ever reaches the detector. This proves a read failure leaves the session
/// able to serve the next call, and nothing more.
///
/// The neighbouring claim, that a failed `Detector::new` is retried rather than
/// cached as a poison value, is verified by inspection rather than by test: the
/// error arm in `clean` returns without assigning `state.detector`, which
/// therefore stays `None` for the next call. It cannot be tested here because
/// `Detector::new` is fallible only under `--features ner` with the recognition
/// model installed, and no CI job has that model.
#[test]
fn a_read_failure_does_not_end_the_session() {
    let workspace = Workspace::new();
    let missing = workspace.path().join("nowhere.txt");
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "Call Marie on 06 12 34 56 78.").expect("writing the note");

    let replies = session(
        &workspace,
        &[
            &call("clean", &format!(r#"{{"path":"{}"}}"#, missing.display())),
            &call("clean", &format!(r#"{{"path":"{}"}}"#, file.display())),
        ],
    );

    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(
        replies[1]["result"]["isError"], false,
        "a failed read must not stop the next call from being served: {:?}",
        replies[1]
    );
}

#[test]
fn clean_without_a_path_is_a_tool_error() {
    let workspace = Workspace::new();
    let replies = session(&workspace, &[&call("clean", "{}")]);
    assert_eq!(replies[0]["result"]["isError"], true);
}

#[test]
fn a_sheet_named_after_a_person_is_headed_with_a_placeholder() {
    // `redact_filenames` defaults to true, so no configuration file is needed
    // to exercise this.
    let workspace = Workspace::new();
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(&book, &[("06 12 34 56 78", &[&["a value"]])]);

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, book.display()),
        )],
    );

    let heading = text_of(&replies[0]);
    assert!(
        !heading.contains("06 12 34 56 78"),
        "the sheet name leaked in the heading:\n{heading}"
    );
    assert!(
        heading.contains("## [[PHONE_1]]"),
        "the heading should carry the placeholder:\n{heading}"
    );
}

/// `redact_filenames` is a filesystem setting: on the command line it decides
/// whether a raw sheet name becomes part of an output filename on the user's
/// own disk. Over MCP the heading goes into the model's context instead, so the
/// setting must not reach it. Without this the coupling comes back unnoticed.
#[test]
fn a_sheet_heading_is_cleaned_even_when_filename_redaction_is_off() {
    let workspace = Workspace::new();
    // Written into `home()`, not `path()`: `path()` is the command's working
    // directory, so a configuration there is found by discovery with or without
    // `--config` and this would prove nothing about the flag.
    let config = workspace.home().join("oboro.toml");
    // An array of tables, not a list of strings. See `testdata/oboro.toml`.
    std::fs::write(
        &config,
        "redact_filenames = false\n\n[[denylist]]\nterm = \"Globex\"\nkind = \"company\"\n",
    )
    .expect("writing the configuration");
    let book = workspace.path().join("book.xlsx");
    support::write_xlsx(&book, &[("Globex", &[&["a value"]])]);

    let replies = session_with(
        &workspace,
        &[
            "--unconfined",
            "--config",
            config.to_str().expect("a UTF-8 path"),
        ],
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, book.display()),
        )],
    );

    let heading = text_of(&replies[0]);
    assert!(
        !heading.contains("Globex"),
        "the sheet name reached the model with filename redaction off:\n{heading}"
    );
    assert!(
        heading.contains("## [[ORG_1]]"),
        "the heading should carry the placeholder:\n{heading}"
    );
}

/// A `.docx` whose only text run holds a bare entity reference, which
/// `src/convert/xml.rs` refuses by quoting the fragment it could not expand.
///
/// The entity must sit inside `<w:t>`: the reader's general-reference arm is
/// depth-guarded, so an entity outside a run is ignored and the call would
/// succeed instead. That makes this fixture self-checking, since a mistake in
/// its construction shows up as an unexpected success.
fn write_docx_with_a_bare_entity(path: &std::path::Path) {
    let document = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:t>Marie &secretfragment; Dupont</w:t></w:r></w:p></w:body>
</w:document>"#;
    let file = std::fs::File::create(path).expect("creating the archive");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("word/document.xml", options)
        .expect("starting the part");
    std::io::Write::write_all(&mut zip, document.as_bytes()).expect("writing the part");
    zip.finish().expect("finishing the archive");
}

#[test]
fn a_reader_error_never_carries_a_fragment_of_the_document() {
    let workspace = Workspace::new();
    let file = workspace.path().join("broken.docx");
    write_docx_with_a_bare_entity(&file);

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, file.display()),
        )],
    );

    assert_eq!(
        replies[0]["result"]["isError"], true,
        "the reader should have refused this document: {:?}",
        replies[0]
    );
    let message = text_of(&replies[0]);
    assert!(
        !message.contains("secretfragment"),
        "a fragment of the document reached the model:\n{message}"
    );
}

#[test]
fn an_unsupported_extension_names_what_can_be_read() {
    let workspace = Workspace::new();
    let file = workspace.path().join("archive.tar.gz");
    std::fs::write(&file, b"not a document").expect("writing the file");

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, file.display()),
        )],
    );

    let message = text_of(&replies[0]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(
        message.contains("docx"),
        "the supported set should be named:\n{message}"
    );
}

#[test]
fn a_missing_path_says_so_rather_than_saying_nothing() {
    let workspace = Workspace::new();
    let missing = workspace.path().join("nowhere.txt");

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, missing.display()),
        )],
    );

    let message = text_of(&replies[0]);
    assert!(
        message.contains("does not exist"),
        "the model cannot act on a vague message:\n{message}"
    );
}

/// The positive half of the encoding classification, end to end. Without this
/// only the negative is covered, and a branch that never fires would pass every
/// other test in this file.
#[test]
fn a_text_file_that_is_not_utf8_is_named_as_such() {
    let workspace = Workspace::new();
    let file = workspace.path().join("latin1.txt");
    // `caf` then 0xe9, which is Latin-1 for the acute e and never valid UTF-8.
    std::fs::write(&file, b"caf\xe9").expect("writing the file");

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, file.display()),
        )],
    );

    let message = text_of(&replies[0]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(
        message.contains("not valid UTF-8"),
        "the encoding branch never fired:\n{message}"
    );
}

#[test]
fn a_corrupt_archive_is_not_called_an_encoding_problem() {
    let workspace = Workspace::new();
    let file = workspace.path().join("corrupt.docx");
    std::fs::write(&file, b"this is not a zip archive at all").expect("writing the file");

    let replies = session(
        &workspace,
        &[&call(
            "clean",
            &format!(r#"{{"path":"{}"}}"#, file.display()),
        )],
    );

    let message = text_of(&replies[0]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(
        !message.contains("UTF-8"),
        "a corrupt archive is not an encoding fault:\n{message}"
    );
}

#[test]
fn a_placeholder_issued_by_clean_is_listed_by_map_list() {
    let workspace = Workspace::new();
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "Call Marie on 06 12 34 56 78.").expect("writing the note");

    let replies = session(
        &workspace,
        &[
            &call("clean", &format!(r#"{{"path":"{}"}}"#, file.display())),
            &call("map_list", "{}"),
        ],
    );

    let listing = text_of(&replies[1]);
    assert!(
        listing.contains("[[PHONE_1]]"),
        "the placeholder clean issued is missing:\n{listing}"
    );
    assert!(
        !listing.contains("06 12 34 56 78"),
        "the value reached the model:\n{listing}"
    );
}

#[test]
fn a_named_configuration_is_honoured_and_its_absence_is_not_silent() {
    let workspace = Workspace::new();
    // Written into `home()`, not `path()`: the command runs with `path()` as
    // its working directory, and `discover_from_cwd` walks upward from there,
    // so a config placed in `path()` would be found with or without
    // `--config` and the second half of this test would prove nothing.
    let config = workspace.home().join("oboro.toml");
    // An array of tables, not a list of strings. See `testdata/oboro.toml`.
    std::fs::write(
        &config,
        "[[denylist]]\nterm = \"Globex\"\nkind = \"company\"\n",
    )
    .expect("writing the configuration");
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "The client is Globex.").expect("writing the note");
    let message = call("clean", &format!(r#"{{"path":"{}"}}"#, file.display()));

    let with = session_with(
        &workspace,
        &[
            "--unconfined",
            "--config",
            config.to_str().expect("a UTF-8 path"),
        ],
        &[&message],
    );
    assert!(
        !text_of(&with[0]).contains("Globex"),
        "the denylisted term survived --config:\n{}",
        text_of(&with[0])
    );

    // The same server without it finds no configuration, because the working
    // directory an MCP client chooses is not the user's project. This is the
    // regression --config exists to prevent, and it is silent without a test.
    let without = session(&workspace, &[&message]);
    assert!(
        text_of(&without[0]).contains("Globex"),
        "this test no longer proves anything: the term was redacted without --config"
    );
}

/// Runs a session expected to fail before serving anything, returning stderr.
fn refused_start(workspace: &Workspace, args: &[&str]) -> String {
    let mut command = workspace.command();
    command.arg("mcp");
    for arg in args {
        command.arg(arg);
    }
    let output = command
        .write_stdin(String::new())
        .output()
        .expect("running oboro mcp");
    assert!(
        !output.status.success(),
        "the server started when it should have refused"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_server_given_neither_roots_nor_unconfined_refuses_to_start() {
    // The point of the whole flag pair: reading the whole disk is not
    // something to arrive at by leaving an argument off.
    let workspace = Workspace::new();
    let reason = refused_start(&workspace, &[]);
    assert!(reason.contains("--root"), "{reason}");
    assert!(reason.contains("--unconfined"), "{reason}");
}

#[test]
fn a_root_that_is_not_a_directory_stops_the_server_at_startup() {
    let workspace = Workspace::new();
    let file = workspace.path().join("note.txt");
    std::fs::write(&file, "value").expect("writing the note");

    let reason = refused_start(
        &workspace,
        &["--root", file.to_str().expect("a UTF-8 path")],
    );
    assert!(reason.contains("not a directory"), "{reason}");

    let missing = workspace.path().join("nowhere");
    let reason = refused_start(
        &workspace,
        &["--root", missing.to_str().expect("a UTF-8 path")],
    );
    assert!(reason.contains("root"), "{reason}");
}

#[test]
fn a_file_inside_a_root_is_cleaned_and_one_outside_is_refused() {
    let workspace = Workspace::new();
    let root = workspace.path().join("work");
    std::fs::create_dir(&root).expect("creating the root");
    let inside = root.join("note.txt");
    std::fs::write(&inside, "Call Marie on 06 12 34 56 78.").expect("writing the note");
    let outside = workspace.path().join("secret.txt");
    std::fs::write(&outside, "Call Marie on 06 12 34 56 78.").expect("writing the secret");

    let replies = session_with(
        &workspace,
        &["--root", root.to_str().expect("a UTF-8 path")],
        &[
            &call("clean", &format!(r#"{{"path":"{}"}}"#, inside.display())),
            &call("clean", &format!(r#"{{"path":"{}"}}"#, outside.display())),
        ],
    );

    assert_eq!(replies[0]["result"]["isError"], false);
    assert!(text_of(&replies[0]).contains("[[PHONE_1]]"));

    assert_eq!(
        replies[1]["result"]["isError"], true,
        "a file outside every root must be refused"
    );
    let refusal = text_of(&replies[1]);
    assert!(refusal.contains("outside"), "{refusal}");
    assert!(
        !refusal.contains("06 12 34 56 78"),
        "the refused file's contents leaked:\n{refusal}"
    );
}

#[test]
fn a_refusal_outside_the_roots_does_not_say_whether_the_file_exists() {
    // A model that could tell these two apart could map the disk outside the
    // roots by asking, which is most of what the roots are for.
    let workspace = Workspace::new();
    let root = workspace.path().join("work");
    std::fs::create_dir(&root).expect("creating the root");
    let real = workspace.path().join("real.txt");
    std::fs::write(&real, "value").expect("writing the file");
    let absent = workspace.path().join("absent.txt");

    let replies = session_with(
        &workspace,
        &["--root", root.to_str().expect("a UTF-8 path")],
        &[
            &call("clean", &format!(r#"{{"path":"{}"}}"#, real.display())),
            &call("clean", &format!(r#"{{"path":"{}"}}"#, absent.display())),
        ],
    );

    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(replies[1]["result"]["isError"], true);
    assert_eq!(
        text_of(&replies[0]).replace("real.txt", "X"),
        text_of(&replies[1]).replace("absent.txt", "X"),
        "the two refusals differ, so existence is disclosed"
    );
}
