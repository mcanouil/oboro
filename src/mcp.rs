//! A Model Context Protocol server speaking JSON-RPC 2.0 over standard input
//! and output.
//!
//! This implements the `2025-11-25` revision and nothing later. The
//! `2026-07-28` revision removes the `initialize` handshake and `ping`, and
//! requires a `server/discover` method, none of which this does, so answering
//! with that version would be a false claim of support.
//!
//! The protocol is hand-rolled rather than taken from the `rmcp` SDK, whose
//! mandatory `tokio` dependency would introduce an async runtime to a crate
//! that has none.

use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config::Config;
use crate::vault::Vault;

/// Serves the protocol until `input` reaches end of file.
///
/// The configuration is borrowed rather than owned because [`crate::detect::Detector`]
/// holds a reference to one, and a detector built lazily beside an owned
/// configuration would be self-referential.
///
/// # Errors
///
/// Returns an error only when reading `input` or writing `output` fails. A
/// tool that fails answers with an error result and the loop continues, since
/// one unreadable file is not a reason to drop the connection.
pub fn serve(
    input: impl BufRead,
    mut output: impl Write,
    config: &Config,
    vault: Vault,
) -> Result<()> {
    let _ = (config, vault);
    for line in input.lines() {
        let line = line.context("reading a message from standard input")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line) {
            writeln!(output, "{reply}").context("writing a message to standard output")?;
            output.flush().context("flushing standard output")?;
        }
    }
    Ok(())
}

/// Answers one message, or `None` when it is a notification and must not be
/// answered.
fn handle(line: &str) -> Option<String> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(rendered(&error(
            None,
            -32700,
            "the message is not valid JSON",
        )));
    };

    // Batching was removed in `2025-06-18`, which is this server's negotiation
    // floor, so a batch should never arrive. It is refused explicitly all the
    // same: the notification rule below would otherwise swallow an array
    // silently and hang a client waiting for replies that never come.
    if message.is_array() {
        return Some(rendered(&error(
            None,
            -32600,
            "JSON-RPC batching was removed in the 2025-06-18 revision; send one message per line",
        )));
    }

    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // Before the method match, not after: a notification naming an unknown
    // method must still go unanswered.
    let id = message.get("id")?;

    let reply = match method {
        "initialize" => match message.get("params") {
            Some(Value::Object(params)) => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                success(
                    id,
                    &json!({
                        "protocolVersion": negotiated(requested),
                        "capabilities": {"tools": {}},
                        "serverInfo": {
                            "name": "oboro",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                    }),
                )
            }
            _ => error(Some(id), -32602, "initialize needs a params object"),
        },
        "ping" => success(id, &json!({})),
        _ => error(Some(id), -32601, &format!("unknown method {method:?}")),
    };
    Some(rendered(&reply))
}

/// A successful reply carrying `result`.
fn success(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// A failed reply.
///
/// The `id` is omitted entirely when it could not be read, rather than sent as
/// null. The protocol diverges from base JSON-RPC here: "Unlike base JSON-RPC,
/// the ID MUST NOT be `null`", so a null would fail validation in a strict
/// client.
fn error(id: Option<&Value>, code: i64, message: &str) -> Value {
    let mut reply = json!({"jsonrpc": "2.0", "error": {"code": code, "message": message}});
    if let Some(id) = id {
        reply["id"] = id.clone();
    }
    reply
}

/// The revision this server implements.
///
/// Not `2026-07-28`, which removes the `initialize` handshake and `ping` and
/// requires a `server/discover` method: a server built like this one does not
/// implement it, and saying otherwise would leave the client expecting a
/// stateless server.
const LATEST: &str = "2025-11-25";

/// The revisions this server answers to as themselves.
///
/// The floor is `2025-06-18` rather than `2024-11-05` because `2025-03-26` and
/// earlier permit JSON-RPC batching, which this server refuses.
const SUPPORTED: [&str; 2] = ["2025-06-18", "2025-11-25"];

/// The version to answer an `initialize` asking for `requested`.
///
/// The rule of record: "Otherwise, the server MUST respond with another
/// protocol version it supports. This SHOULD be the latest version supported
/// by the server."
fn negotiated(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|asked| SUPPORTED.iter().find(|known| **known == asked).copied())
        .unwrap_or(LATEST)
}

/// Renders a reply as one line.
///
/// Compact rather than pretty: the transport forbids embedded newlines, and
/// the pretty form has them.
fn rendered(reply: &Value) -> String {
    serde_json::to_string(reply).unwrap_or_else(|_| {
        // Every value reaching here is built from `json!` literals and strings,
        // none of which can fail to serialise. A hand-written fallback keeps
        // this total rather than panicking in a server.
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"the reply could not be serialised"}}"#
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_answered_with_its_own_id() {
        let reply = handle(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["id"], 7);
        assert_eq!(value["jsonrpc"], "2.0");
    }

    #[test]
    fn a_string_id_comes_back_as_a_string() {
        let reply = handle(r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["id"], "abc");
    }

    #[test]
    fn a_notification_is_not_answered() {
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn an_unknown_notification_is_not_answered_either() {
        assert!(handle(r#"{"jsonrpc":"2.0","method":"who/knows"}"#).is_none());
    }

    #[test]
    fn a_line_that_is_not_json_is_a_parse_error_with_no_id() {
        let reply = handle("not json at all").expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32700);
        assert!(
            value.get("id").is_none(),
            "the id field must be absent, not null: {value}"
        );
    }

    #[test]
    fn a_batch_is_rejected_rather_than_ignored() {
        let reply = handle(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32600);
        assert!(
            value.get("id").is_none(),
            "the id field must be absent: {value}"
        );
    }

    #[test]
    fn an_unknown_method_is_a_method_not_found() {
        let reply =
            handle(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn a_reply_never_contains_a_newline() {
        let reply = handle(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).expect("a reply");
        assert!(!reply.contains('\n'), "framing would break: {reply}");
    }

    #[test]
    fn the_latest_supported_version_is_offered_by_default() {
        assert_eq!(negotiated(None), "2025-11-25");
    }

    #[test]
    fn a_supported_version_is_echoed() {
        assert_eq!(negotiated(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiated(Some("2025-11-25")), "2025-11-25");
    }

    #[test]
    fn a_newer_version_is_answered_with_the_one_this_server_implements() {
        // 2026-07-28 removes the handshake and `ping` and requires
        // `server/discover`, so echoing it would claim support that does not exist.
        assert_eq!(negotiated(Some("2026-07-28")), "2025-11-25");
    }

    #[test]
    fn a_version_below_the_floor_is_answered_with_the_latest() {
        // The floor is 2025-06-18 because 2025-03-26 permits batching.
        assert_eq!(negotiated(Some("2025-03-26")), "2025-11-25");
        assert_eq!(negotiated(Some("2024-11-05")), "2025-11-25");
    }

    #[test]
    fn initialize_advertises_tools_and_names_the_server() {
        let reply =
            handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#)
                .expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["result"]["protocolVersion"], "2025-11-25");
        assert!(value["result"]["capabilities"]["tools"].is_object());
        assert_eq!(value["result"]["serverInfo"]["name"], "oboro");
        assert_eq!(
            value["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn initialize_without_params_is_an_invalid_params_error() {
        let reply = handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32602);
    }

    #[test]
    fn ping_takes_no_params_and_does_not_demand_them() {
        let reply = handle(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(value["result"].is_object());
        assert!(value.get("error").is_none());
    }
}
