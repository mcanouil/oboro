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
fn session(workspace: &Workspace, messages: &[&str]) -> Vec<serde_json::Value> {
    let output = workspace
        .command()
        .arg("mcp")
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
