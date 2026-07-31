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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::config::Config;
use crate::detect::Detector;
use crate::vault::{Entry, Vault};
use crate::{convert, pipeline};

/// Which files `clean` will open.
///
/// Naming roots is the default and reading anywhere has to be asked for, the
/// other way round from the command line. The caller here is a model rather
/// than the person at the keyboard, and a client that offers "always allow" on
/// a tool call turns one careless approval into a standing licence to read the
/// whole disk. Documenting that was not enough.
pub enum Roots {
    /// Read any file the user running the server can read.
    Unconfined,
    /// Read only within these directories, already canonicalised.
    Within(Vec<PathBuf>),
}

impl Roots {
    /// Confines reading to `roots`.
    ///
    /// They are canonicalised once, here, so that a link or a `..` cannot walk
    /// out of one later, and so a root that does not exist is reported while
    /// there is still a person watching rather than as a refusal to every call.
    ///
    /// # Errors
    ///
    /// Returns an error if a root does not exist or is not a directory.
    pub fn within(roots: &[PathBuf]) -> Result<Self> {
        let mut canonical = Vec::with_capacity(roots.len());
        for root in roots {
            let resolved = root
                .canonicalize()
                .with_context(|| format!("reading the root {}", root.display()))?;
            if !resolved.is_dir() {
                bail!("the root {} is not a directory", root.display());
            }
            canonical.push(resolved);
        }
        Ok(Self::Within(canonical))
    }

    /// Why the file at `resolved` may not be read, or `None` when it may.
    ///
    /// `shown` is the path as the model wrote it and appears in the message;
    /// `resolved` is what [`resolved`] made of it and is the only thing the
    /// decision rests on. They are separate arguments because the caller must
    /// go on to read `resolved` rather than `shown`: reading what the model
    /// wrote would let the kernel answer questions this refusal will not.
    ///
    /// The wording never says whether the path exists. A model that could tell
    /// a refusal for a real file from one for an imagined file could map the
    /// disk outside the roots by asking, which is most of what the roots are
    /// for.
    fn refusal(&self, shown: &Path, resolved: &Path) -> Option<String> {
        let Self::Within(roots) = self else {
            return None;
        };
        // `starts_with` compares whole components, so the root `/tmp/work`
        // does not admit `/tmp/workshop`.
        if roots.iter().any(|root| resolved.starts_with(root)) {
            return None;
        }
        Some(format!(
            "{} is outside the directories this server was given; \
             it reads only within: {}",
            shown.display(),
            roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// `path` with `.` and `..` removed, without touching the filesystem.
///
/// Folding has to happen before anything is looked up, or which components
/// exist decides where a `..` lands. It is not a sound substitute for
/// `canonicalize` in general, since `link/..` is not `.` when `link` is a
/// symbolic link, and it is only ever used below on a path the kernel has
/// already refused to resolve.
fn folded(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            // Popping the root leaves the root, so `/..` stays `/`.
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` with every link and `..` resolved, as far as it exists.
///
/// `canonicalize` fails outright on a path whose last component is missing,
/// which is the ordinary case for a typo, so the deepest part that does exist
/// is resolved and the rest re-attached. That gives a path comparable against a
/// root without first asking whether the file is there, which is what keeps the
/// containment check ahead of anything that could disclose existence.
///
/// The fold is what makes that true rather than merely intended. An earlier
/// version walked up the path as written, and `Path::file_name` returns `None`
/// for `..`, so those components were dropped instead of applied. Two paths
/// naming the same file then resolved differently depending on whether an
/// unrelated directory in the middle existed, and the model could read the
/// difference: one call per probe told it whether any directory on the machine
/// existed. Folding first makes containment a function of the string and the
/// roots alone.
///
/// The remaining gap is closed by the kernel rather than here. A tail
/// re-attached after the fold has not had its links resolved, so it can compare
/// as inside a root while naming something outside one; but this branch is
/// reached only when `canonicalize` failed, and `realpath` needs strictly less
/// than `open`, so a path that gets that far cannot be read either.
fn resolved(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    if let Ok(canonical) = absolute.canonicalize() {
        return canonical;
    }

    let folded = folded(&absolute);
    let mut trailing = Vec::new();
    let mut head = folded.as_path();
    while let Some(parent) = head.parent() {
        // Safe now that the fold has removed every `..`, which is the one
        // component `file_name` cannot name.
        if let Some(name) = head.file_name() {
            trailing.push(name.to_os_string());
        }
        if let Ok(canonical) = parent.canonicalize() {
            let mut out = canonical;
            out.extend(trailing.iter().rev());
            return out;
        }
        head = parent;
    }
    folded
}

/// What the server carries between messages.
///
/// The detector is built on the first `clean` rather than at startup: under
/// `--features ner` building one loads a 348 MB model, and a client that
/// connects and lists tools without cleaning anything should not pay for it.
struct State<'a> {
    config: &'a Config,
    vault: Vault,
    detector: Option<Detector<'a>>,
    roots: Roots,
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
    roots: Roots,
) -> Result<()> {
    let mut state = State {
        config,
        vault,
        detector: None,
        roots,
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

/// Whether this build can read `path`'s file type.
///
/// [`convert::supported`] rather than [`convert::format_of`] alone: `format_of`
/// maps an image extension to [`convert::Format::Image`] whatever the build,
/// while `supported` drops images unless the `ocr` feature is compiled in, and
/// it is off by default. Going by `format_of` on a default build would let a
/// `.png` through to `convert::read` and answer with the generic message, when
/// the message here names the real reason.
fn reads(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| convert::supported().iter().any(|known| *known == extension))
}

/// Why `path` cannot be read, when that can be established without reading it.
///
/// The three cases here, an unreadable file type, a directory, and a path that
/// is missing or sits behind an unsearchable parent, are all settled before
/// `convert::read` runs, so none of them has to recover anything from an error
/// chain.
fn unreadable(path: &Path) -> Option<String> {
    if !reads(path) {
        return Some(format!(
            "{} is not a file type Oboro reads; this build reads: {}",
            path.display(),
            convert::supported().join(", ")
        ));
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Some(format!(
            "{} is a directory; name a single file",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Some(format!("{} does not exist", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Some(format!(
            "{} cannot be read: permission denied",
            path.display()
        )),
        // A readable file, or a `metadata` failure this cannot name. Both fall
        // through to `convert::read`, which is better placed to say what went
        // wrong than a guess made before the file was opened.
        Ok(_) | Err(_) => None,
    }
}

/// Whether any cause in `error` is an [`std::io::Error`] of `kind`.
///
/// The chain is walked and downcast rather than matched on its message: the
/// message is the thing that cannot be trusted, since readers quote fragments
/// of the document into it, and such a fragment is precisely what must not
/// reach a reply.
fn has_io_kind(error: &anyhow::Error, kind: std::io::ErrorKind) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == kind)
    })
}

/// Whether `error` is a text encoding fault rather than anything else.
///
/// Gated on the format because `Text`, `Csv` and `Tsv` are exactly the formats
/// [`convert::read`] sends through `read_utf8`, so they are the only ones for
/// which "is not valid UTF-8 text" says anything true. Telling a model that
/// about a `.pdf` would be meaningless whatever error arrived, and the gate
/// keeps the predicate sound should a reader ever surface `InvalidData` from a
/// decompressor.
fn is_encoding_fault(error: &anyhow::Error, format: convert::Format) -> bool {
    matches!(
        format,
        convert::Format::Text | convert::Format::Csv | convert::Format::Tsv
    ) && has_io_kind(error, std::io::ErrorKind::InvalidData)
}

/// Reads a file and returns its cleaned text, one content block per part.
///
/// A workbook yields one part per sheet, so a heading keeps them apart for the
/// clients that flatten the content array into one string.
///
/// That heading is cleaned whatever `redact_filenames` says, unlike the command
/// line, which honours the setting. The setting is documented as a filesystem
/// concern: it decides whether a raw sheet name becomes part of an output
/// filename on the user's own disk, where the user can see it and chose it.
/// Here the destination is the model's context instead, and a sheet name may
/// hold PII, so letting a flag about filenames put a raw name in front of a
/// model would defeat the one thing this server exists to do.
fn clean(path: &str, state: &mut State<'_>) -> Value {
    let shown = Path::new(path);
    // Resolved once, and everything after this uses it rather than what the
    // model wrote. Reading the original string would hand the kernel the job
    // of resolving `..`, and its answer depends on which directories along the
    // way exist, which is exactly the question the roots refuse to answer.
    let resolved = resolved(shown);
    let path = resolved.as_path();

    // Before everything else, including the checks that would say whether the
    // file is there. A refusal for being outside the roots must look the same
    // whether or not the path exists, or a model could map the disk by asking.
    if let Some(reason) = state.roots.refusal(shown, path) {
        return failed(&reason);
    }
    if let Some(reason) = unreadable(path) {
        return failed(&reason);
    }
    // `unreadable` has already established that the extension is one Oboro
    // reads, so this cannot be `None`.
    let format = convert::format_of(path).expect("the format was checked");

    let parts = match convert::read(path, &state.config.ocr_languages) {
        Ok(conversion) => conversion.into_parts(),
        Err(error) => {
            crate::note!("oboro mcp: reading {} failed: {error:#}", path.display());
            // Permission first, and ungated by format: `std::fs::metadata`
            // needs no read permission on the file itself, only a searchable
            // parent, so a file the server may not read passes the pre-flight
            // and only fails here. Asked before the encoding case because a
            // file that could not be opened says nothing about its encoding.
            return failed(
                &if has_io_kind(&error, std::io::ErrorKind::PermissionDenied) {
                    format!("{} cannot be read: permission denied", path.display())
                } else if is_encoding_fault(&error, format) {
                    format!("{} is not valid UTF-8 text", path.display())
                } else {
                    format!(
                        "{} could not be read as {format:?}; it may hold no extractable text, \
                     or it may be damaged. The reason is on the server's standard error.",
                        path.display()
                    )
                },
            );
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
                     the reason is on the server's standard error",
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
            Some((_, name)) => match pipeline::clean(&name, detector, &mut state.vault) {
                Ok(report) => Some(report.text),
                Err(error) => {
                    crate::note!("oboro mcp: cleaning a sheet name failed: {error:#}");
                    return failed(
                        "the file could not be cleaned; the reason is on the server's standard error",
                    );
                }
            },
            None => None,
        };
        match pipeline::clean(&text, detector, &mut state.vault) {
            Ok(report) => blocks.push(match heading {
                Some(heading) => format!("## {heading}\n\n{}", report.text),
                None => report.text,
            }),
            Err(error) => {
                crate::note!("oboro mcp: cleaning {} failed: {error:#}", path.display());
                return failed(
                    "the file could not be cleaned; the reason is on the server's standard error",
                );
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
            failed("the vault could not be read; the reason is on the server's standard error")
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

    /// `Roots::refusal` over a path as written, resolving it the way `clean`
    /// does. The two arguments exist so the caller can read the resolved form;
    /// the tests only care about the decision.
    impl Roots {
        fn refusal_for(&self, path: &Path) -> Option<String> {
            self.refusal(path, &resolved(path))
        }
    }

    /// A server state over a vault in `dir`, reading anywhere.
    fn state<'a>(dir: &std::path::Path, config: &'a Config) -> State<'a> {
        State {
            config,
            vault: Vault::open(&dir.join("vault.db"), &dir.join("key")).expect("a vault"),
            detector: None,
            roots: Roots::Unconfined,
        }
    }

    #[test]
    fn an_unconfined_server_admits_any_path() {
        assert!(
            Roots::Unconfined
                .refusal_for(Path::new("/anywhere/at/all.txt"))
                .is_none()
        );
    }

    #[test]
    fn a_path_inside_a_root_is_admitted() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let roots = Roots::within(&[dir.path().to_path_buf()]).expect("a root");
        assert!(roots.refusal_for(&dir.path().join("note.txt")).is_none());
        assert!(
            roots
                .refusal_for(&dir.path().join("deeper/note.txt"))
                .is_none()
        );
    }

    #[test]
    fn a_path_outside_every_root_is_refused() {
        let inside = tempfile::tempdir().expect("temporary directory");
        let outside = tempfile::tempdir().expect("temporary directory");
        let roots = Roots::within(&[inside.path().to_path_buf()]).expect("a root");
        assert!(
            roots
                .refusal_for(&outside.path().join("secret.txt"))
                .is_some()
        );
    }

    #[test]
    fn a_sibling_sharing_a_prefix_is_not_inside_the_root() {
        // `/tmp/work` must not admit `/tmp/workshop`. A textual prefix test
        // would; comparing whole components is what stops it.
        let dir = tempfile::tempdir().expect("temporary directory");
        let root = dir.path().join("work");
        std::fs::create_dir(&root).expect("creating the root");
        std::fs::create_dir(dir.path().join("workshop")).expect("creating the sibling");
        let roots = Roots::within(&[root]).expect("a root");
        assert!(
            roots
                .refusal_for(&dir.path().join("workshop/secret.txt"))
                .is_some()
        );
    }

    #[test]
    fn a_traversal_out_of_a_root_is_refused() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let root = dir.path().join("work");
        std::fs::create_dir(&root).expect("creating the root");
        let roots = Roots::within(std::slice::from_ref(&root)).expect("a root");
        assert!(roots.refusal_for(&root.join("../secret.txt")).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_leading_out_of_a_root_is_refused() {
        // The reason roots are canonicalised rather than compared as written:
        // a link inside the root is a path out of it.
        let dir = tempfile::tempdir().expect("temporary directory");
        let root = dir.path().join("work");
        std::fs::create_dir(&root).expect("creating the root");
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "value").expect("writing the secret");
        std::os::unix::fs::symlink(&secret, root.join("link.txt")).expect("linking");

        let roots = Roots::within(std::slice::from_ref(&root)).expect("a root");
        assert!(roots.refusal_for(&root.join("link.txt")).is_some());
    }

    #[test]
    fn a_traversal_lands_in_the_same_place_whatever_exists_around_it() {
        // The regression that matters. `Path::file_name` returns `None` for
        // `..`, so an earlier version dropped those components while walking
        // up, and where a path landed depended on whether an unrelated
        // directory in the middle of it existed. A model could read that
        // difference and use it to ask whether any directory on the machine
        // was there, one call at a time.
        let dir = tempfile::tempdir().expect("temporary directory");
        let root = dir.path().join("work");
        std::fs::create_dir(&root).expect("creating the root");
        std::fs::create_dir(dir.path().join("present")).expect("creating the probe");
        let roots = Roots::within(std::slice::from_ref(&root)).expect("a root");

        let through_present = dir.path().join("present/../work/absent.txt");
        let through_absent = dir.path().join("missing/../work/absent.txt");
        assert_eq!(
            resolved(&through_present),
            resolved(&through_absent),
            "where a path lands must not depend on what exists around it"
        );
        assert!(roots.refusal_for(&through_present).is_none());
        assert!(
            roots.refusal_for(&through_absent).is_none(),
            "the probe directory's absence changed the answer"
        );
    }

    #[test]
    fn folding_removes_dot_and_parent_components() {
        assert_eq!(folded(Path::new("/a/./b/../c")), Path::new("/a/c"));
        // Popping the root leaves the root rather than escaping above it.
        assert_eq!(folded(Path::new("/../../a")), Path::new("/a"));
    }

    #[test]
    fn a_root_that_is_not_a_directory_is_refused_at_startup() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "value").expect("writing the file");
        assert!(Roots::within(&[file]).is_err());
        assert!(Roots::within(&[dir.path().join("missing")]).is_err());
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

    /// An `anyhow` chain carrying an `InvalidData` io error, which is the shape
    /// the text reader produces on a file that is not valid UTF-8.
    fn invalid_data_chain() -> anyhow::Error {
        anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        ))
        .context("reading a file")
    }

    #[test]
    fn an_invalid_data_error_on_a_text_format_is_an_encoding_fault() {
        for format in [
            convert::Format::Text,
            convert::Format::Csv,
            convert::Format::Tsv,
        ] {
            assert!(
                is_encoding_fault(&invalid_data_chain(), format),
                "{format:?} should have been recognised"
            );
        }
    }

    /// The gate, tested directly, because it cannot be reached end to end: no
    /// reader in this build surfaces `InvalidData` from an archive, since every
    /// one of them fails with a `ZipError` or a `Utf8Error` and neither is an
    /// `std::io::Error`. The gate is kept regardless, because "is not valid
    /// UTF-8 text" is only a true thing to say about the formats read through
    /// `read_utf8`, whatever error happens to arrive for the others.
    #[test]
    fn the_same_error_on_an_archive_format_is_not_an_encoding_fault() {
        for format in [
            convert::Format::Docx,
            convert::Format::Odt,
            convert::Format::Pptx,
            convert::Format::Xlsx,
            convert::Format::Pdf,
            convert::Format::Image,
        ] {
            assert!(
                !is_encoding_fault(&invalid_data_chain(), format),
                "{format:?} was mislabelled an encoding problem"
            );
        }
    }

    #[test]
    fn an_error_with_no_io_cause_is_not_an_encoding_fault() {
        let error = anyhow::anyhow!("the document uses an entity this reader cannot expand");
        assert!(!is_encoding_fault(&error, convert::Format::Text));
    }

    #[test]
    fn an_unsupported_extension_is_refused_before_the_file_is_read() {
        // No file is created: the extension settles it, so a path that does not
        // exist must still be refused for its type rather than its absence.
        let reason = unreadable(std::path::Path::new("/nowhere/archive.tar.gz"))
            .expect("an unsupported extension must be refused");
        assert!(
            reason.contains("docx"),
            "the supported set is missing: {reason}"
        );
    }

    #[test]
    fn a_directory_is_told_apart_from_a_file() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("notes.txt");
        std::fs::create_dir(&path).expect("creating the directory");
        let reason = unreadable(&path).expect("a directory must be refused");
        assert!(reason.contains("directory"), "{reason}");
    }

    #[test]
    fn a_readable_file_has_no_reason_to_refuse_it() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "text").expect("writing the note");
        assert!(unreadable(&path).is_none());
    }

    /// `format_of` maps an image extension to a format on every build, but
    /// `supported` only lists images when the `ocr` feature is compiled in.
    /// Going by the former would let a `.png` reach `convert::read` on a
    /// default build and answer with the generic message, when the model can be
    /// told plainly that this build does not read it.
    #[test]
    fn an_image_is_refused_unless_this_build_can_recognise_text_in_one() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("photo.png");
        std::fs::write(&path, b"not really a png").expect("writing the file");

        assert_eq!(
            reads(&path),
            convert::ocr_available(),
            "`reads` must follow `supported`, which follows the `ocr` feature"
        );
        assert_eq!(unreadable(&path).is_some(), !convert::ocr_available());
    }

    #[test]
    fn an_extension_is_matched_whatever_its_case() {
        assert!(reads(std::path::Path::new("/tmp/REPORT.DOCX")));
        assert!(!reads(std::path::Path::new("/tmp/archive.tar.gz")));
        assert!(!reads(std::path::Path::new("/tmp/no_extension")));
    }

    /// `std::fs::metadata` needs no read permission on the file itself, so a
    /// file the server may not read passes the pre-flight and can only be
    /// recognised from the chain afterwards.
    #[test]
    fn a_permission_denied_cause_is_found_in_the_chain() {
        let error = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "permission denied",
        ))
        .context("opening a file");
        assert!(has_io_kind(&error, std::io::ErrorKind::PermissionDenied));
        assert!(!has_io_kind(&error, std::io::ErrorKind::InvalidData));
        assert!(
            !is_encoding_fault(&error, convert::Format::Text),
            "a file that could not be opened says nothing about its encoding"
        );
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
