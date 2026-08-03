//! Email message text extraction.
//!
//! An `.eml` is an RFC 5322 message: headers, then one or more MIME parts.
//! The headers are where the personal data is densest, and they are read
//! before any body is.
//!
//! The file is read as bytes rather than as a string. An email is bytes with
//! a declared charset, and a message a client wrote in ISO-8859-1 would be
//! refused outright by a UTF-8 read.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use mail_parser::{Address, Message, MessageParser, MimeHeaders, PartType};
use regex::Regex;

/// Any tag, with its name captured.
///
/// `html_to_text` breaks a line on `<br>` and a closing `</p>` and on nothing
/// else, so every other element is stripped with no separator and its text
/// welds onto its neighbour's. Gmail composes one `<div>` per line with no
/// whitespace between the tags, so this is the common case rather than an odd
/// one, and a welded value matches no rule while no longer matching its
/// planted spelling either: it leaks with the leak test passing.
static TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<\s*/?\s*([a-z][a-z0-9]*)").expect("tag pattern is valid"));

/// The tags whose text runs on with the text either side of them.
///
/// This is the way round that fails safe, and it is the same conclusion
/// `src/convert/odt.rs` reached: a tag nobody thought of costs a line break
/// rather than letting two values weld into one that matches no rule. Naming
/// the block tags instead means enumerating a list nobody finishes, and
/// `<center>`, which is ordinary in real mail, was already missing from it.
///
/// The listed tags have to keep concatenating: `Jean <b>Dupont</b>` is one
/// name, and splitting it stops it matching, which is the same rule
/// `src/convert/xml.rs` depends on.
const INLINE_TAGS: &[&str] = &[
    "a", "abbr", "acronym", "b", "bdi", "bdo", "big", "cite", "code", "data", "del", "dfn", "em",
    "font", "i", "img", "ins", "kbd", "label", "mark", "nobr", "q", "rp", "rt", "ruby", "s",
    "samp", "small", "span", "strike", "strong", "sub", "sup", "time", "tt", "u", "var", "wbr",
];

/// A `mailto:` or `tel:` link target.
///
/// `html_to_text` strips attributes, so an address written only as a link
/// target reaches the detectors as the link's own text and the address itself
/// is lost.
static LINK_TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<a\b[^>]*\bhref\s*=\s*["'](?:mailto:|tel:)([^"']*)["'][^>]*>"#)
        .expect("link target pattern is valid")
});

/// How deep a chain of forwarded messages is followed.
///
/// `mail-parser` has its own `MAX_NESTED_ENCODED` of 3, but that bounds only
/// base64 or quoted-printable encoded nested messages, which come back as
/// `Binary` beyond it rather than as a `Message`. Unencoded nesting takes a
/// different path inside the parser with no cap at all, so the cap has to be
/// ours. Eight is well past any real thread and far short of anything that
/// threatens the stack.
const MAX_FORWARD_DEPTH: usize = 8;

/// How many nested messages a file may declare before it is refused unread.
///
/// [`MAX_FORWARD_DEPTH`] bounds this module's own walk, which is not the same
/// thing: `MessageParser::parse` builds the whole tree first, and dropping a
/// chain of nested messages recurses once per level. Measured, a file of some
/// eighty thousand stacked `message/rfc822` parts overflows the stack as the
/// parsed message goes out of scope, and a stack overflow aborts the process
/// rather than returning an error, so a directory walk would die on the file
/// with no diagnostic. Counting the declarations in the bytes, before they are
/// parsed, is what keeps that shape from being built at all. A thousand is
/// past any real thread by orders of magnitude and short of the overflow by
/// nearly two.
const MAX_NESTED_MESSAGES: usize = 1024;

/// The content type a message declares for a part it nests inside itself.
const NESTED_MESSAGE_TYPE: &[u8] = b"message/rfc822";

/// Reads a message: its human-written headers, then its bodies.
///
/// # Errors
///
/// Returns an error if the file cannot be read, if no header parses so the
/// file is not a message at all, or if it yields no text.
pub fn to_text(path: &Path) -> Result<String> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

    let nested = raw
        .windows(NESTED_MESSAGE_TYPE.len())
        .filter(|window| window.eq_ignore_ascii_case(NESTED_MESSAGE_TYPE))
        .count();
    if nested > MAX_NESTED_MESSAGES {
        bail!(
            "{} declares {nested} nested messages, more than the {MAX_NESTED_MESSAGES} \
             this reads; parsing it would exhaust the stack rather than fail",
            path.display()
        );
    }

    let message = MessageParser::default()
        .parse(&raw)
        .with_context(|| format!("{} is not a readable .eml message", path.display()))?;

    let mut unread = 0;
    let text = render(&message, 0, &mut unread, path)?;

    if text.trim().is_empty() {
        bail!(
            "{} contains no extractable text; if its content is attachments, \
             read those separately",
            path.display()
        );
    }
    if unread > 0 {
        crate::note!("{}: {unread} attachment(s) not read", path.display());
    }
    Ok(text)
}

/// Renders one message: headers, what it carries, then every body it shows.
///
/// The bodies are taken from `parts` directly rather than from `text_body` and
/// `html_body`. Those two lists are the parser's answer to "what would a mail
/// client display", which is a narrower question than this one, and they leave
/// text out in shapes that occur in ordinary post: a `multipart/related` whose
/// HTML root is not its first part is left out of both, and so is any part the
/// parser could not decode, since failing to decode one also demotes it out of
/// them. Reading every part instead means a body cannot be missed by being
/// classified as something else, and it makes the twin problem disappear
/// rather than needing a rule: an ordinary message names the same part id in
/// both lists, but it is still one part, so walking parts emits it once.
fn render(message: &Message, depth: usize, unread: &mut usize, path: &Path) -> Result<String> {
    let mut out = headers(message);

    // `attachments()` yields nested messages and demoted bodies too, so both
    // have to be recognised here or a forward is announced as unread while
    // being followed below, and a body is announced instead of being read.
    for part in message
        .attachments()
        .filter(|part| !part.is_message() && !is_body(part))
    {
        *unread += 1;
        announce(&mut out, part);
    }

    for part in &message.parts {
        if !is_body(part) {
            continue;
        }
        if part.is_encoding_problem {
            let kind = part.content_type().map_or_else(
                || "unknown type".to_owned(),
                |content| content.ctype().to_owned(),
            );
            bail!(
                "{}: the encoding of a {} body part could not be decoded, so the \
                 message would be read with less text than it holds",
                path.display(),
                super::quoted(&kind)
            );
        }
        match &part.body {
            PartType::Text(text) => push_block(&mut out, text),
            PartType::Html(html) => push_block(&mut out, &html_text(html)),
            _ => {}
        }
    }

    for nested in message
        .attachments()
        .filter_map(mail_parser::MessagePart::message)
    {
        if depth >= MAX_FORWARD_DEPTH {
            *unread += 1;
            push_block(
                &mut out,
                "Forwarded message: not read, more than eight levels of forwarding",
            );
            continue;
        }
        push_block(&mut out, "Forwarded message:");
        push_block(&mut out, &render(nested, depth + 1, unread, path)?);
    }

    Ok(out)
}

/// Whether a part is text the message shows rather than a file it carries.
///
/// A filename is what separates the two, and it is the parser's own answer to
/// a `Content-Disposition: attachment` or a `name` parameter. A `text/plain`
/// sent as an attachment therefore stays an attachment, named and not read,
/// which is the answer every other attachment gets.
fn is_body(part: &mail_parser::MessagePart) -> bool {
    matches!(part.body, PartType::Text(_) | PartType::Html(_)) && part.attachment_name().is_none()
}

/// Writes the line naming a part that is carried rather than read.
///
/// A part with no filename is named by its type alone. Announcing it is what
/// keeps a meeting invitation, which is as dense with personal data as an
/// email gets and carries no filename at all, from leaving nothing in the
/// output to say the message carried anything.
fn announce(out: &mut String, part: &mail_parser::MessagePart) {
    let kind = part.content_type().map_or_else(
        || "unknown type".to_owned(),
        |content| match content.subtype() {
            Some(subtype) => format!("{}/{subtype}", content.ctype()),
            None => content.ctype().to_owned(),
        },
    );

    out.push_str("Attachment: ");
    match part.attachment_name() {
        Some(name) => out.push_str(printable(name).trim()),
        None => out.push_str("unnamed"),
    }
    out.push_str(" (");
    out.push_str(printable(&kind).trim());
    out.push_str(")\n");
}

/// Flattens HTML to text, after making its block structure survivable.
///
/// The separator inserted before a block tag is a `<br>` rather than a plain
/// newline, since `html_to_text` collapses whitespace between tags into a
/// single space. A space keeps two values from welding but leaves them on one
/// line, where a rule matching one can run on into the next: measured on the
/// fixture, one address placeholder swallowed the telephone number sitting in
/// the next `<div>`. A `<br>` is the one thing the flattener turns into a real
/// line break.
fn html_text(html: &str) -> String {
    let linked = LINK_TARGET.replace_all(html, "$0 $1 ");
    let broken = TAG.replace_all(&linked, |captured: &regex::Captures| {
        let whole = &captured[0];
        if INLINE_TAGS.contains(&captured[1].to_ascii_lowercase().as_str()) {
            whole.to_owned()
        } else {
            format!("<br>{whole}")
        }
    });
    mail_parser::decoders::html::html_to_text(&broken)
}

/// Appends a block, keeping a blank line between it and whatever precedes it
/// so two bodies never run together into one candidate value.
fn push_block(out: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text.trim_end());
    out.push('\n');
}

/// The headers a person wrote and read.
///
/// Routing headers are deliberately absent: `Received`, `Message-ID`, DKIM
/// signatures and `X-*` are the envelope rather than the message. Dropping one
/// is not a leak, since a header never rendered never reaches the model; it
/// makes the output faithful to the message rather than to the file.
fn headers(message: &Message) -> String {
    let mut out = String::new();
    if let Some(date) = message.date() {
        header_line(&mut out, "Date", &date.to_rfc822());
    }
    address_line(&mut out, "From", message.from());
    address_line(&mut out, "Reply-To", message.reply_to());
    address_line(&mut out, "To", message.to());
    address_line(&mut out, "Cc", message.cc());
    address_line(&mut out, "Bcc", message.bcc());
    if let Some(subject) = message.subject() {
        header_line(&mut out, "Subject", subject);
    }
    out
}

fn header_line(out: &mut String, name: &str, value: &str) {
    let value = printable(value);
    if value.trim().is_empty() {
        return;
    }
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value.trim());
    out.push('\n');
}

/// Renders an address header as `Name <address>` pairs.
///
/// `Address::iter` flattens a group into its members and drops the group's own
/// display name, which can be human-written. That loss is accepted: the
/// members are the addresses, and the alternative is parsing the raw header
/// again by hand.
fn address_line(out: &mut String, name: &str, address: Option<&Address>) {
    let Some(address) = address else {
        return;
    };
    let rendered: Vec<String> = address
        .iter()
        .map(
            |addr| match (addr.name.as_deref(), addr.address.as_deref()) {
                (Some(display), Some(email)) => {
                    format!(
                        "{} <{}>",
                        printable(display).trim(),
                        printable(email).trim()
                    )
                }
                (None, Some(value)) | (Some(value), None) => printable(value).trim().to_owned(),
                (None, None) => String::new(),
            },
        )
        .filter(|entry| !entry.is_empty())
        .collect();

    if rendered.is_empty() {
        return;
    }
    out.push_str(name);
    out.push_str(": ");
    out.push_str(&rendered.join(", "));
    out.push('\n');
}

/// Replaces control characters with a space.
///
/// RFC 2047 can encode any byte, so a decoded header value may carry CR, LF or
/// ESC. A line terminator would split a header line in two and an escape
/// sequence would reach the written file. Replacing rather than deleting keeps
/// two values either side of a stripped character from welding into one.
fn printable(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `raw` to a temporary `.eml` and reads it back.
    fn read(raw: &str) -> Result<String> {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("message.eml");
        std::fs::write(&path, raw).expect("writing");
        to_text(&path)
    }

    #[test]
    fn headers_and_a_plain_body_are_read() {
        let text = read(
            "Date: Tue, 3 Feb 2026 09:12:44 +0100\r\n\
             From: Jean Dupont <jean.dupont@acme-consulting.example>\r\n\
             To: Marie Martin <marie.martin@globex.example>\r\n\
             Subject: Contrat CT-874512\r\n\
             \r\n\
             Body line 06 12 34 56 78\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("From: Jean Dupont <jean.dupont@acme-consulting.example>"),
            "{text}"
        );
        assert!(
            text.contains("To: Marie Martin <marie.martin@globex.example>"),
            "{text}"
        );
        assert!(text.contains("Subject: Contrat CT-874512"), "{text}");
        assert!(text.contains("Body line 06 12 34 56 78"), "{text}");
    }

    /// RFC 2047 can encode any byte, so a decoded subject may carry CR or LF.
    /// Left alone it would break the `Subject:` line in two, and an ESC would
    /// reach the written file.
    #[test]
    fn control_characters_in_a_decoded_header_do_not_break_the_line() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: =?UTF-8?B?QQ0KV0lQRTogeA==?=\r\n\
             \r\n\
             body\r\n",
        )
        .expect("reading");

        assert!(
            !text.contains("A\r"),
            "a carriage return survived:\n{text:?}"
        );
        assert!(
            text.lines()
                .filter(|line| line.starts_with("Subject:"))
                .count()
                == 1,
            "{text}"
        );
    }

    /// A group is flattened to its members. The group's own display name is
    /// dropped by `Address::iter`, which is accepted rather than worked around.
    #[test]
    fn an_address_group_is_flattened_to_its_members() {
        let text = read(
            "From: a@b.example\r\n\
             To: Team: alice@x.example, bob@y.example;\r\n\
             Subject: S\r\n\
             \r\n\
             body\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("To: alice@x.example, bob@y.example"),
            "{text}"
        );
    }

    /// Every header a person wrote is rendered, not only the four an ordinary
    /// message carries. A recipient in `Cc` or `Bcc` is a person the message
    /// went to, and a `Reply-To` is an address someone chose to publish.
    #[test]
    fn every_rendered_header_is_read() {
        let text = read(
            "Date: Tue, 3 Feb 2026 09:12:44 +0100\r\n\
             From: a@b.example\r\n\
             Reply-To: reply@d.example\r\n\
             To: to@e.example\r\n\
             Cc: cc@f.example\r\n\
             Bcc: bcc@g.example\r\n\
             Subject: S\r\n\
             \r\n\
             body\r\n",
        )
        .expect("reading");

        for expected in [
            "Date: Tue, 3 Feb 2026",
            "Reply-To: reply@d.example",
            "Cc: cc@f.example",
            "Bcc: bcc@g.example",
        ] {
            assert!(text.contains(expected), "{expected} is missing:\n{text}");
        }
    }

    /// A telephone number written only as a link target is lost with the
    /// attributes unless it is lifted out first, the same as a `mailto:`.
    #[test]
    fn a_tel_link_target_is_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <a href=\"tel:+33142685300\">call us</a>\r\n",
        )
        .expect("reading");

        assert!(text.contains("+33142685300"), "{text}");
    }

    /// A control character is replaced by a space rather than deleted, so two
    /// values either side of one cannot weld into a third that matches no
    /// rule. Deleting would pass the sibling test, which only counts lines.
    #[test]
    fn a_control_character_leaves_a_space_behind_rather_than_welding() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: =?UTF-8?B?MDYgMTIgMzQgNTYgNzgNNzUwMDIgUGFyaXM=?=\r\n\
             \r\n\
             body\r\n",
        )
        .expect("reading");

        assert!(
            !text.contains("7875002"),
            "a stripped control character welded two values:\n{text}"
        );
    }

    /// A message with nothing but an empty body and headers that render to
    /// nothing must say so rather than write an empty file that reads as a
    /// document with nothing sensitive in it.
    #[test]
    fn a_message_with_no_text_at_all_is_refused() {
        let error = read("Message-ID: <x@y.example>\r\n\r\n\r\n").expect_err("must refuse");
        assert!(
            format!("{error:#}").contains("no extractable text"),
            "{error:#}"
        );
    }

    /// The parser is best-effort and returns `None` only when no header parses
    /// at all, so this fixture must carry no colon on its first line.
    #[test]
    fn a_file_that_is_not_a_message_is_reported_clearly() {
        let error = read("not an email at all\n").expect_err("must reject");
        assert!(format!("{error:#}").contains("readable .eml message"));
    }

    #[test]
    fn a_quoted_printable_body_is_rejoined_across_a_soft_break() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/plain\r\n\
             Content-Transfer-Encoding: quoted-printable\r\n\
             \r\n\
             phone 06 12 =\r\n\
             34 56 78 end\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("06 12 34 56 78"),
            "a soft line break split the number:\n{text}"
        );
    }

    #[test]
    fn an_encoded_word_subject_is_decoded() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: =?UTF-8?Q?Contrat_CT-874512_soci=C3=A9t=C3=A9?=\r\n\
             \r\n\
             body\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("Subject: Contrat CT-874512 société"),
            "{text}"
        );
    }

    /// A display name is encoded the same way a subject is, and it is where a
    /// person's name sits.
    #[test]
    fn an_encoded_word_in_a_display_name_is_decoded() {
        let text = read(
            "From: =?UTF-8?Q?Jean_Dup=C3=B4nt?= <jean.dupont@acme-consulting.example>\r\n\
             Subject: S\r\n\
             \r\n\
             body\r\n",
        )
        .expect("reading");

        assert!(text.contains("Jean Dupônt"), "{text}");
    }

    #[test]
    fn a_base64_body_is_decoded() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/plain\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             MDYgMTIgMzQgNTYgNzg=\r\n",
        )
        .expect("reading");

        assert!(text.contains("06 12 34 56 78"), "{text}");
    }

    /// A mail client writes ISO-8859-1 and says so. Reading the file as a
    /// UTF-8 string first would refuse it outright.
    #[test]
    fn a_single_byte_charset_body_is_decoded() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("latin.eml");
        let mut raw = b"From: a@b.example\r\nSubject: S\r\n\
                        Content-Type: text/plain; charset=iso-8859-1\r\n\r\n"
            .to_vec();
        raw.extend_from_slice(&[0x53, 0x6f, 0x63, 0x69, 0xe9, 0x74, 0xe9, 0x0d, 0x0a]);
        std::fs::write(&path, &raw).expect("writing");

        assert!(to_text(&path).expect("reading").contains("Société"));
    }

    /// A multi-byte charset read as lossy UTF-8 comes back with characters
    /// replaced, which is a document read for less than it holds and a name
    /// that no longer matches the denylist entry written to catch it.
    #[test]
    fn a_multi_byte_charset_body_is_decoded() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("shift-jis.eml");
        let mut raw = b"From: a@b.example\r\nSubject: S\r\n\
                        Content-Type: text/plain; charset=shift_jis\r\n\r\n"
            .to_vec();
        // "東京" in Shift-JIS.
        raw.extend_from_slice(&[0x93, 0x8c, 0x8b, 0x9e, 0x0d, 0x0a]);
        std::fs::write(&path, &raw).expect("writing");

        let text = to_text(&path).expect("reading");
        assert!(text.contains("東京"), "{text:?}");
        assert!(
            !text.contains('\u{fffd}'),
            "characters were replaced:\n{text:?}"
        );
    }

    /// A plain-text message parses with the same part id in both `text_body`
    /// and `html_body`, and `body_html` then returns HTML the parser
    /// synthesised. Reading every text body and then every HTML body would
    /// emit an ordinary email twice.
    #[test]
    fn an_ordinary_message_body_is_emitted_exactly_once() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             \r\n\
             Body line 06 12 34 56 78\r\n",
        )
        .expect("reading");

        assert_eq!(
            text.matches("Body line").count(),
            1,
            "the body was emitted more than once:\n{text}"
        );
    }

    /// The plain twin is not reliably a superset: a client may send a
    /// "requires HTML" stub as `text/plain`, and every address in the HTML
    /// would be missed.
    #[test]
    fn both_alternative_twins_are_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/alternative; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             plain twin 06 12 34 56 78\r\n\
             --X\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <p>html twin 75002 Paris</p>\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(text.contains("plain twin 06 12 34 56 78"), "{text}");
        assert!(text.contains("html twin 75002 Paris"), "{text}");
        assert!(!text.contains("<p>"), "tags survived:\n{text}");
    }

    /// `html_to_text` breaks a line on `<br>` and `</p>` and on nothing else,
    /// so adjacent block elements weld together. Gmail composes one `<div>`
    /// per line with no whitespace between tags, and a welded value matches no
    /// rule while no longer matching its planted spelling either, so it leaks
    /// with the leak test passing.
    #[test]
    fn adjacent_block_elements_do_not_merge() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <div>06 12 34 56 78</div><div>75002 Paris</div>\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("06 12 34 56 78"),
            "the number was welded:\n{text}"
        );
        assert!(
            text.contains("75002 Paris"),
            "the postcode was welded:\n{text}"
        );
        assert!(
            !text.contains("7875002"),
            "two values merged into one candidate:\n{text}"
        );
        assert!(
            !text
                .lines()
                .any(|line| line.contains("06 12 34 56 78") && line.contains("75002")),
            "two blocks share a line, where one rule can run on into the next:\n{text}"
        );
    }

    /// The tags that lay a message out are not a list anybody finishes, and a
    /// tag nobody thought of welded its text onto its neighbour's, which is
    /// the leak this whole rule exists to stop. `<center>` is ordinary in real
    /// mail; `tbody`, `figcaption` and `fieldset` are the same shape.
    #[test]
    fn a_block_tag_nobody_enumerated_still_breaks_the_line() {
        for (open, close) in [
            ("<center>", "</center>"),
            ("<tbody>", "</tbody>"),
            ("<figcaption>", "</figcaption>"),
            ("<fieldset>", "</fieldset>"),
        ] {
            let text = read(&format!(
                "From: a@b.example\r\n\
                 Subject: S\r\n\
                 Content-Type: text/html\r\n\
                 \r\n\
                 {open}06 12 34 56 78{close}{open}Acme Consulting SARL{close}\r\n"
            ))
            .expect("reading");

            assert!(
                !text.contains("78Acme"),
                "{open} welded two values into one candidate:\n{text}"
            );
        }
    }

    #[test]
    fn adjacent_table_cells_do_not_merge() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <table><tr><td>Acme Consulting SARL</td><td>Globex Industries</td></tr></table>\r\n",
        )
        .expect("reading");

        assert!(
            !text.contains("SARLGlobex"),
            "two cells merged into one candidate:\n{text}"
        );
    }

    /// Inline emphasis must still concatenate, or a name split across two tags
    /// stops matching. This is the same rule the shared XML run reader keeps.
    #[test]
    fn inline_tags_still_concatenate() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <p>Jean <b>Dupont</b> at <i>12</i> bis rue de la Paix</p>\r\n",
        )
        .expect("reading");

        assert!(text.contains("Jean Dupont"), "{text}");
        assert!(text.contains("12 bis rue de la Paix"), "{text}");
    }

    /// `html_to_text` strips attributes, so an address written only as a link
    /// target is otherwise lost entirely.
    #[test]
    fn a_mailto_link_target_is_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <a href=\"mailto:jean.dupont@acme-consulting.example\">contact</a>\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("jean.dupont@acme-consulting.example"),
            "{text}"
        );
    }

    #[test]
    fn character_entities_are_decoded() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <p>8 avenue des Champs-&Eacute;lys&eacute;es</p>\r\n",
        )
        .expect("reading");

        assert!(text.contains("8 avenue des Champs-Élysées"), "{text}");
    }

    /// Both parts sit in `text_body`, so a reader taking only the first would
    /// silently drop the second.
    #[test]
    fn two_plain_parts_are_both_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             first 06 12 34 56 78\r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             second 75002 Paris\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(text.contains("first 06 12 34 56 78"), "{text}");
        assert!(text.contains("second 75002 Paris"), "{text}");
    }

    /// The filename goes into the text so it is cleaned like any other value,
    /// and it is often the most identifying thing about an attachment. The
    /// bytes are never read.
    #[test]
    fn an_attachment_is_named_but_not_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             body\r\n\
             --X\r\n\
             Content-Type: application/pdf; name=\"contrat.pdf\"\r\n\
             Content-Disposition: attachment; filename=\"contrat.pdf\"\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             U0VDUkVUQllURVM=\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(text.contains("Attachment: contrat.pdf"), "{text}");
        assert!(
            !text.contains("SECRETBYTES"),
            "attachment bytes reached the output:\n{text}"
        );
    }

    #[test]
    fn an_rfc_2231_encoded_filename_is_decoded() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             body\r\n\
             --X\r\n\
             Content-Type: application/pdf\r\n\
             Content-Disposition: attachment; filename*=UTF-8''contrat-%C3%A9t%C3%A9.pdf\r\n\
             \r\n\
             bytes\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(text.contains("contrat-été.pdf"), "{text}");
    }

    #[test]
    fn two_attachments_sharing_a_filename_both_appear() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             body\r\n\
             --X\r\n\
             Content-Type: application/pdf\r\n\
             Content-Disposition: attachment; filename=\"scan.pdf\"\r\n\
             \r\n\
             one\r\n\
             --X\r\n\
             Content-Type: application/pdf\r\n\
             Content-Disposition: attachment; filename=\"scan.pdf\"\r\n\
             \r\n\
             two\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert_eq!(text.matches("Attachment: scan.pdf").count(), 2, "{text}");
    }

    /// A text part marked as an attachment is treated as an attachment, named
    /// and not read, which is the same answer every other attachment gets. Its
    /// content never reaches the output, so it is not a leak, only a document
    /// read less fully than the file holds.
    #[test]
    fn a_text_part_marked_as_an_attachment_is_named_not_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             body\r\n\
             --X\r\n\
             Content-Type: text/plain; name=\"notes.txt\"\r\n\
             Content-Disposition: attachment; filename=\"notes.txt\"\r\n\
             \r\n\
             attached text 4242 4242 4242 4242\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(text.contains("Attachment: notes.txt"), "{text}");
        assert!(!text.contains("4242 4242 4242 4242"), "{text}");
    }

    /// A forwarded message is the densest personal data an inbox holds, and
    /// `attachments()` yields it alongside real attachments, so it has to be
    /// recognised before it is counted as one.
    #[test]
    fn a_forwarded_message_is_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: Fwd\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             see below\r\n\
             --X\r\n\
             Content-Type: message/rfc822\r\n\
             \r\n\
             From: inner@c.example\r\n\
             Subject: Inner\r\n\
             \r\n\
             inner body 4242 4242 4242 4242\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(text.contains("Forwarded message:"), "{text}");
        assert!(text.contains("From: inner@c.example"), "{text}");
        assert!(text.contains("inner body 4242 4242 4242 4242"), "{text}");
        assert!(
            !text.contains("Attachment:"),
            "a forward was also counted as an unread attachment:\n{text}"
        );
    }

    /// A part whose transfer encoding will not decode yields text short of
    /// what it holds, which is the silent under-read `src/convert/mod.rs`
    /// exists to refuse. It fails only for a part being read as a body; an
    /// attachment is already not read, so its encoding is beside the point.
    #[test]
    fn a_body_that_will_not_decode_fails_rather_than_reading_short() {
        let error = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: text/plain\r\n\
             Content-Transfer-Encoding: base64\r\n\
             \r\n\
             ****not base64****\r\n",
        )
        .expect_err("must refuse");

        assert!(format!("{error:#}").contains("could not be decoded"));
    }

    /// An email whose only content is an attachment still has headers, and
    /// they are the part carrying the addresses, so it must not be refused as
    /// holding no text.
    #[test]
    fn an_attachment_only_message_still_returns_its_headers() {
        let text = read(
            "From: Jean Dupont <jean.dupont@acme-consulting.example>\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: application/pdf\r\n\
             Content-Disposition: attachment; filename=\"contrat.pdf\"\r\n\
             \r\n\
             bytes\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("jean.dupont@acme-consulting.example"),
            "{text}"
        );
        assert!(text.contains("Attachment: contrat.pdf"), "{text}");
    }

    /// A part the parser could not decode is demoted out of `text_body` and
    /// into `attachments`, so a reader that checks the encoding only on the
    /// body lists never sees the one part that has the problem. This shape is
    /// what a truncated download produces: the closing boundary is missing.
    #[test]
    fn a_body_demoted_to_an_attachment_still_fails_rather_than_reading_short() {
        let error = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             first 06 12 34 56 78\r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             second jean.dupont@acme-consulting.example\r\n",
        )
        .expect_err("must refuse");

        assert!(
            format!("{error:#}").contains("could not be decoded"),
            "{error:#}"
        );
    }

    /// `mail-parser` picks a `multipart/related` root by position and ignores
    /// the RFC 2387 `start` parameter, so an HTML root that is not the first
    /// part is left out of both body lists. Reading only those lists loses the
    /// entire body of such a message while still returning its headers, which
    /// is output that looks sanitised without having been read.
    #[test]
    fn an_html_root_that_is_not_the_first_part_is_read() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/related; boundary=X; type=\"text/html\"\r\n\
             \r\n\
             --X\r\n\
             Content-Type: image/png\r\n\
             \r\n\
             png\r\n\
             --X\r\n\
             Content-Type: text/html\r\n\
             \r\n\
             <p>Jean Dupont 06 12 34 56 78</p>\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("Jean Dupont 06 12 34 56 78"),
            "the body was lost:\n{text}"
        );
    }

    /// An attachment with no filename is the shape a meeting invitation takes,
    /// and it is as dense with personal data as an email gets. Counted but
    /// never named, it leaves nothing in the output saying the message carried
    /// anything at all.
    #[test]
    fn an_unnamed_attachment_is_still_announced() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: Invite\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             covering note\r\n\
             --X\r\n\
             Content-Type: application/ics; method=REQUEST\r\n\
             Content-Disposition: attachment\r\n\
             \r\n\
             BEGIN:VCALENDAR\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(
            text.contains("Attachment:"),
            "an unnamed part left no trace:\n{text}"
        );
        assert!(text.contains("application/ics"), "{text}");
    }

    /// A content type is written by whoever produced the message, so it may
    /// carry any byte. Unlike a filename it was reaching the output as found,
    /// putting an escape sequence into a file the user is expected to paste
    /// elsewhere.
    #[test]
    fn a_hostile_content_type_cannot_reach_the_output_as_written() {
        let text = read(
            "From: a@b.example\r\n\
             Subject: S\r\n\
             Content-Type: multipart/mixed; boundary=X\r\n\
             \r\n\
             --X\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             body\r\n\
             --X\r\n\
             Content-Type: \x1b[2Japp/\x1b[31mevil; name=\"f.bin\"\r\n\
             Content-Disposition: attachment; filename=\"f.bin\"\r\n\
             \r\n\
             bytes\r\n\
             --X--\r\n",
        )
        .expect("reading");

        assert!(
            !text.contains('\x1b'),
            "an escape sequence reached the output:\n{text:?}"
        );
    }

    /// Past the depth cap the forward was counted and nothing was written, so
    /// the output gave no sign a thread had been cut short.
    #[test]
    fn a_forward_past_the_depth_cap_is_announced() {
        let mut raw = String::from("From: a@b.example\r\nSubject: S\r\n");
        for _ in 0..12 {
            raw.push_str("Content-Type: message/rfc822\r\n\r\nFrom: b@c.example\r\n");
        }
        raw.push_str("\r\ninner 4242 4242 4242 4242\r\n");

        let text = read(&raw).expect("reading");

        assert!(
            text.contains("not read"),
            "a forward past the cap left no trace:\n{text}"
        );
    }

    /// The depth cap bounds this module's own walk, not the tree `mail-parser`
    /// builds and then destroys: dropping a long chain of nested messages
    /// recurses once per level and overflows the stack, which aborts the
    /// process rather than returning an error. The shape has to be refused
    /// before it is parsed.
    #[test]
    fn a_file_of_stacked_nested_messages_is_refused_before_it_is_parsed() {
        let mut raw = String::from("From: a@b.example\r\nSubject: S\r\n");
        for _ in 0..=MAX_NESTED_MESSAGES {
            raw.push_str("Content-Type: message/rfc822\r\n\r\nFrom: b@c.example\r\n");
        }
        raw.push_str("\r\nbody\r\n");

        let error = read(&raw).expect_err("must refuse");
        assert!(
            format!("{error:#}").contains("nested messages"),
            "{error:#}"
        );
    }
}
