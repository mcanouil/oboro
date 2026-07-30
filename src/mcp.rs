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
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config::Config;
use crate::detect::Detector;
use crate::vault::{Entry, Vault};
use crate::{convert, pipeline};

/// What the server carries between messages.
///
/// The detector is built on the first `clean` rather than at startup: under
/// `--features ner` building one loads a 348 MB model, and a client that
/// connects and lists tools without cleaning anything should not pay for it.
struct State<'a> {
    config: &'a Config,
    vault: Vault,
    detector: Option<Detector<'a>>,
}

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
    let mut state = State {
        config,
        vault,
        detector: None,
    };
    for line in input.lines() {
        let line = line.context("reading a message from standard input")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line, &mut state) {
            writeln!(output, "{reply}").context("writing a message to standard output")?;
            output.flush().context("flushing standard output")?;
        }
    }
    Ok(())
}

/// Answers one message, or `None` when it is a notification and must not be
/// answered.
fn handle(line: &str, state: &mut State<'_>) -> Option<String> {
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
        // No `-32602` guard: `tools/list` params are optional, and a client may
        // send a `cursor`, which this server ignores because it returns every
        // tool in one page.
        "tools/list" => success(id, &json!({"tools": descriptors()})),
        "tools/call" => match message.get("params") {
            Some(Value::Object(params)) => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let empty = json!({});
                let arguments = params.get("arguments").unwrap_or(&empty);
                success(id, &call(name, arguments, state))
            }
            _ => error(Some(id), -32602, "tools/call needs a params object"),
        },
        "ping" => success(id, &json!({})),
        _ => error(Some(id), -32601, &format!("unknown method {method:?}")),
    };
    Some(rendered(&reply))
}

/// The tools this server offers, in a fixed order.
///
/// Two, not the three the roadmap named. `restore` is deliberately absent:
/// over MCP the caller is the model, and a model that can write a file of
/// placeholders and read it back afterwards can use `restore` to obtain every
/// value the vault holds, which is what the vault exists to prevent.
fn descriptors() -> Value {
    json!([
        {
            "name": "clean",
            "description": "Read a file and return its text with sensitive values replaced by \
                            stable placeholders such as [[NAME_1]] and [[PHONE_2]]. Prefer this \
                            over reading a file directly: it keeps names, addresses, telephone \
                            numbers, e-mail addresses and account identifiers out of your \
                            context. It also reads .pdf, .docx, .xlsx, .pptx and .odt, which a \
                            plain file read cannot open at all.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read and clean.",
                    },
                },
                "required": ["path"],
            },
        },
        {
            "name": "map_list",
            "description": "List the placeholders this vault has issued, so you can tell which \
                            kind of value each one stands for. The real values are never \
                            returned.",
            "inputSchema": {"type": "object", "properties": {}},
        },
    ])
}

/// A successful `tools/call` result, one content block per entry in `blocks`.
fn text_result(blocks: Vec<String>) -> Value {
    let content: Vec<Value> = blocks
        .into_iter()
        .map(|text| json!({"type": "text", "text": text}))
        .collect();
    json!({"content": content, "isError": false})
}

/// A failed `tools/call` result.
///
/// Not a JSON-RPC error: those are for protocol faults, and a tool that ran
/// and failed is something the model should be able to read and act on.
fn failed(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

/// Runs one tool.
fn call(name: &str, arguments: &Value, state: &mut State<'_>) -> Value {
    match name {
        "clean" => match arguments.get("path").and_then(Value::as_str) {
            Some(path) => clean(path, state),
            None => failed("clean needs a `path` argument naming the file to read"),
        },
        "map_list" => map_list(&state.vault),
        // `restore` lands here deliberately. See `descriptors`.
        other => failed(&format!(
            "there is no tool named {other:?}; this server offers clean and map_list"
        )),
    }
}

/// Reads a file and returns its cleaned text, one content block per part.
///
/// A workbook yields one part per sheet, so a heading keeps them apart for the
/// clients that flatten the content array into one string.
fn clean(path: &str, state: &mut State<'_>) -> Value {
    let path = Path::new(path);

    let parts = match convert::read(path, &state.config.ocr_languages) {
        Ok(conversion) => conversion.into_parts(),
        Err(error) => {
            crate::note!("oboro mcp: reading {} failed: {error:#}", path.display());
            return failed("the file could not be read; the reason is in the server's log");
        }
    };

    // Built here rather than at startup, and rebuilt after a failure rather
    // than cached as a poison value.
    if state.detector.is_none() {
        match Detector::new(state.config) {
            Ok(detector) => state.detector = Some(detector),
            // Returning without assigning `state.detector` is the whole of the
            // retry: it stays `None`, so the next call builds again rather than
            // inheriting a poisoned value. Caching the failure here would
            // answer every later call with this message.
            Err(error) => {
                crate::note!("oboro mcp: building the detector failed: {error:#}");
                return failed(
                    "the detection stack could not be built, so nothing was cleaned; \
                     the reason is in the server's log",
                );
            }
        }
    }
    let detector = state
        .detector
        .as_ref()
        .expect("the detector was just built");

    let mut blocks = Vec::with_capacity(parts.len());
    for (sheet, text) in parts {
        let heading = match sheet {
            Some((_, name)) if state.config.redact_filenames => {
                match pipeline::clean(&name, detector, &mut state.vault) {
                    Ok(report) => Some(report.text),
                    Err(error) => {
                        crate::note!("oboro mcp: cleaning a sheet name failed: {error:#}");
                        return failed(
                            "the file could not be cleaned; the reason is in the server's log",
                        );
                    }
                }
            }
            Some((_, name)) => Some(name),
            None => None,
        };
        match pipeline::clean(&text, detector, &mut state.vault) {
            Ok(report) => blocks.push(match heading {
                Some(heading) => format!("## {heading}\n\n{}", report.text),
                None => report.text,
            }),
            Err(error) => {
                crate::note!("oboro mcp: cleaning {} failed: {error:#}", path.display());
                return failed("the file could not be cleaned; the reason is in the server's log");
            }
        }
    }
    text_result(blocks)
}

/// Lists the placeholders the vault has issued.
///
/// Placeholders only. `oboro map list` prints the time each was created and
/// this does not: the model needs to know which placeholders exist and what
/// kind each stands for, and a timestamp per entry is a record of the user's
/// working hours rather than anything it can reason with.
fn map_list(vault: &Vault) -> Value {
    match vault.entries() {
        Ok(entries) if entries.is_empty() => {
            text_result(vec!["the vault holds no placeholders yet".to_owned()])
        }
        Ok(entries) => {
            let listing: Vec<String> = entries.iter().map(Entry::placeholder).collect();
            text_result(vec![listing.join("\n")])
        }
        Err(error) => {
            crate::note!("oboro mcp: listing the vault failed: {error:#}");
            failed("the vault could not be read; the reason is in the server's log")
        }
    }
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

    /// A temporary directory and the default configuration, so no test can
    /// touch the developer's real `~/.oboro`.
    ///
    /// The two are returned rather than folded into [`state`] because the
    /// state borrows the configuration and the directory has to outlive the
    /// vault opened inside it.
    fn fixture() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().expect("temporary directory");
        (dir, Config::load(None).expect("the default configuration"))
    }

    /// A server state over a vault in `dir`.
    fn state<'a>(dir: &std::path::Path, config: &'a Config) -> State<'a> {
        State {
            config,
            vault: Vault::open(&dir.join("vault.db"), &dir.join("key")).expect("a vault"),
            detector: None,
        }
    }

    #[test]
    fn a_request_is_answered_with_its_own_id() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply =
            handle(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#, &mut state).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["id"], 7);
        assert_eq!(value["jsonrpc"], "2.0");
    }

    #[test]
    fn a_string_id_comes_back_as_a_string() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#,
            &mut state,
        )
        .expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["id"], "abc");
    }

    #[test]
    fn a_notification_is_not_answered() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        assert!(
            handle(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &mut state
            )
            .is_none()
        );
    }

    #[test]
    fn an_unknown_notification_is_not_answered_either() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        assert!(handle(r#"{"jsonrpc":"2.0","method":"who/knows"}"#, &mut state).is_none());
    }

    #[test]
    fn a_line_that_is_not_json_is_a_parse_error_with_no_id() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle("not json at all", &mut state).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32700);
        assert!(
            value.get("id").is_none(),
            "the id field must be absent, not null: {value}"
        );
    }

    #[test]
    fn a_batch_is_rejected_rather_than_ignored() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply =
            handle(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#, &mut state).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32600);
        assert!(
            value.get("id").is_none(),
            "the id field must be absent: {value}"
        );
    }

    #[test]
    fn an_unknown_method_is_a_method_not_found() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#,
            &mut state,
        )
        .expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn a_reply_never_contains_a_newline() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply =
            handle(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, &mut state).expect("a reply");
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
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply =
            handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#, &mut state)
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
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            &mut state,
        )
        .expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(value["error"]["code"], -32602);
    }

    #[test]
    fn ping_takes_no_params_and_does_not_demand_them() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply =
            handle(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, &mut state).expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(value["result"].is_object());
        assert!(value.get("error").is_none());
    }

    #[test]
    fn exactly_two_tools_are_offered() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &mut state,
        )
        .expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        let tools = value["result"]["tools"].as_array().expect("an array");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["clean", "map_list"]);
    }

    #[test]
    fn there_is_no_restore_tool() {
        // Restoring over MCP is equivalent to `map list --reveal` for any client
        // that can read a file. If this test fails, read the spec before changing
        // it.
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &mut state,
        )
        .expect("a reply");
        assert!(
            !reply.contains("restore"),
            "restore must not be exposed: {reply}"
        );
    }

    #[test]
    fn the_tool_order_is_fixed() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let once = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &mut state,
        )
        .expect("a reply");
        let twice = handle(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            &mut state,
        )
        .expect("a reply");
        let first: Value = serde_json::from_str(&once).expect("valid JSON");
        let second: Value = serde_json::from_str(&twice).expect("valid JSON");
        assert_eq!(first["result"]["tools"], second["result"]["tools"]);
    }

    #[test]
    fn map_list_offers_no_way_to_reveal_values() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &mut state,
        )
        .expect("a reply");
        assert!(
            !reply.contains("reveal"),
            "the mapping must not be revealable: {reply}"
        );
    }

    #[test]
    fn clean_requires_a_path() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            &mut state,
        )
        .expect("a reply");
        let value: Value = serde_json::from_str(&reply).expect("valid JSON");
        let clean = &value["result"]["tools"][0];
        assert_eq!(clean["inputSchema"]["required"][0], "path");
        assert_eq!(clean["inputSchema"]["properties"]["path"]["type"], "string");
    }

    #[test]
    fn tools_list_tolerates_a_cursor_and_absent_params() {
        let (dir, config) = fixture();
        let mut state = state(dir.path(), &config);
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"cursor":"x"}}"#,
        ] {
            let reply = handle(line, &mut state).expect("a reply");
            let value: Value = serde_json::from_str(&reply).expect("valid JSON");
            assert!(value.get("error").is_none(), "{line} was refused: {reply}");
        }
    }
}
