//! The `PostToolUse` hook, which is the only path where Oboro runs without a
//! person watching.
//!
//! Two things matter here. The model must see placeholders rather than values,
//! and a failure must withhold the tool result rather than let it through: a
//! privacy tool that quietly degrades to no privacy is worse than one that
//! stops.

mod support;

use support::Workspace;

/// The value planted in every payload below.
const VALUE: &str = "06 12 34 56 78";

/// A `PostToolUse` payload carrying `result` as its `tool_result`.
fn payload(result: &str) -> String {
    format!(
        r#"{{"session_id":"s","hook_event_name":"PostToolUse","tool_name":"Read",
             "tool_input":{{"file_path":"/tmp/note.txt"}},"tool_use_id":"t",
             "tool_result":{result},"tool_result_is_error":false}}"#
    )
}

/// Runs the hook with `payload` on standard input, returning its output.
fn run(workspace: &Workspace, args: &[&str], payload: &str) -> std::process::Output {
    let mut command = workspace.command();
    for arg in args {
        command.arg(arg);
    }
    command
        .arg("hook")
        .arg("post-tool-use")
        .write_stdin(payload.to_owned())
        .output()
        .expect("running oboro hook")
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("output must be UTF-8")
}

#[test]
fn a_string_result_comes_back_cleaned() {
    let workspace = Workspace::new();
    let output = run(&workspace, &[], &payload(&format!(r#""Call {VALUE}.""#)));

    assert!(
        output.status.success(),
        "the hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_of(&output);
    assert!(
        !stdout.contains(VALUE),
        "the value reached the model:\n{stdout}"
    );
    assert!(
        stdout.contains("[[PHONE_1]]"),
        "the placeholder is missing:\n{stdout}"
    );
    assert!(
        stdout.contains(r#""hookEventName":"PostToolUse""#)
            && stdout.contains(r#""updatedToolOutput""#),
        "the reply is not shaped as a PostToolUse hook output:\n{stdout}"
    );
}

/// Some tools answer with an object rather than a string. Every string in it
/// has to be cleaned, and its shape has to survive so the model can still read
/// what the tool said.
#[test]
fn a_structured_result_keeps_its_shape_and_is_cleaned() {
    let workspace = Workspace::new();
    let result =
        format!(r#"{{"stdout":"Call {VALUE}.","lines":["also {VALUE}"],"code":0,"empty":null}}"#);
    let output = run(&workspace, &[], &payload(&result));

    assert!(
        output.status.success(),
        "the hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = stdout_of(&output);
    assert!(
        !stdout.contains(VALUE),
        "the value reached the model:\n{stdout}"
    );
    for expected in ["stdout", "lines", "code", "[[PHONE_1]]"] {
        assert!(
            stdout.contains(expected),
            "the structure lost {expected}:\n{stdout}"
        );
    }
}

/// A payload with nothing to clean must say so by changing nothing, rather
/// than replacing a result it never saw.
#[test]
fn a_payload_without_a_result_changes_nothing() {
    let workspace = Workspace::new();
    let output = run(
        &workspace,
        &[],
        r#"{"hook_event_name":"PostToolUse","tool_name":"Read"}"#,
    );

    assert!(
        output.status.success(),
        "the hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "nothing should be written when there is no result: {}",
        stdout_of(&output)
    );
}

#[test]
fn malformed_input_is_withheld_rather_than_passed_through() {
    let workspace = Workspace::new();
    let output = run(&workspace, &[], &format!("not json at all, {VALUE}"));

    assert_eq!(
        output.status.code(),
        Some(0),
        "the reply is only honoured on exit 0, so a failure must still exit 0"
    );
    let stdout = stdout_of(&output);
    assert!(
        !stdout.contains(VALUE),
        "the value reached the model despite the failure:\n{stdout}"
    );
    assert!(
        stdout.contains(r#""decision":"block""#),
        "a failure must block rather than continue:\n{stdout}"
    );
}

/// `kjf3`'s test: break Oboro deliberately and confirm the tool result is
/// withheld instead of reaching the model.
#[test]
fn a_broken_vault_withholds_the_result() {
    let workspace = Workspace::new();
    // A directory where the database belongs, so opening the vault fails
    // however it is reached.
    let [vault, _key] = workspace.store_paths();
    std::fs::create_dir(&vault).expect("creating a directory where a vault is expected");

    let output = run(&workspace, &[], &payload(&format!(r#""Call {VALUE}.""#)));

    assert_eq!(
        output.status.code(),
        Some(0),
        "the reply is only honoured on exit 0, so a failure must still exit 0"
    );
    let stdout = stdout_of(&output);
    assert!(
        !stdout.contains(VALUE),
        "the value reached the model despite the failure:\n{stdout}"
    );
    assert!(
        stdout.contains(r#""decision":"block""#),
        "a failure must block rather than continue:\n{stdout}"
    );
    assert!(
        stdout.contains("updatedToolOutput"),
        "the result must be replaced, not left as it was:\n{stdout}"
    );

    // The failure names the vault path, which is the kind of thing a vault
    // redacts. The user is told; the model is not.
    let (before_message, message) = stdout
        .split_once(r#""systemMessage""#)
        .expect("the reply must tell the user what broke");
    assert!(
        message.contains("vault"),
        "the user must be told what broke:\n{stdout}"
    );
    assert!(
        !before_message.contains("vault"),
        "the model must not be told the path that failed:\n{stdout}"
    );
}

/// What the hook hands the model has to be recoverable, or the answer that
/// comes back cannot be read.
#[test]
fn what_the_hook_returns_restores_to_the_original() {
    let workspace = Workspace::new();
    let output = run(&workspace, &[], &payload(&format!(r#""Call {VALUE}.""#)));
    let stdout = stdout_of(&output);

    let cleaned = stdout
        .split(r#""updatedToolOutput":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the reply must carry updatedToolOutput");
    assert_eq!(
        workspace.restore_piped(cleaned),
        format!("Call {VALUE}."),
        "the placeholders the model saw must restore to the original text"
    );
}
