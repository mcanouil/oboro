//! Email message text extraction.
//!
//! An `.eml` is an RFC 5322 message: headers, then one or more MIME parts.
//! The headers are where the personal data is densest, and they are read
//! before any body is.
//!
//! The file is read as bytes rather than as a string. An email is bytes with
//! a declared charset, and a message a client wrote in ISO-8859-1 would be
//! refused outright by a UTF-8 read.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use mail_parser::{Address, Message, MessageParser, MimeHeaders, PartType};
use regex::Regex;

/// The tags that begin or end a visual block.
///
/// `html_to_text` breaks a line on `<br>` and a closing `</p>` and on nothing
/// else, so without this every other block element is stripped with no
/// separator and its text welds onto its neighbour's. Gmail composes one
/// `<div>` per line with no whitespace between the tags, so this is the common
/// case rather than an odd one.
///
/// Inline tags are deliberately absent: `Jean <b>Dupont</b>` has to keep
/// concatenating, which is the same rule `src/convert/xml.rs` depends on.
static BLOCK_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)<\s*/?\s*(?:div|p|br|tr|td|th|li|ul|ol|dl|dd|dt|h[1-6]|table|blockquote|section|article|header|footer|pre|hr|address)\b",
    )
    .expect("block tag pattern is valid")
});

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

/// Reads a message: its human-written headers, then its bodies.
///
/// # Errors
///
/// Returns an error if the file cannot be read, if no header parses so the
/// file is not a message at all, or if it yields no text.
pub fn to_text(path: &Path) -> Result<String> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
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

/// Renders one message: headers, then every distinct body.
///
/// The body rule is the subtle part. An ordinary message parses with the same
/// part id in both `text_body` and `html_body`, so emitting every text body
/// and then every HTML body would emit it twice, the second time round-tripped
/// through HTML the parser synthesised. Only inside a `multipart/alternative`
/// are the two lists disjoint, which is exactly where both twins are wanted:
/// the plain twin is not reliably a superset, since a client may send a
/// "requires HTML" stub and hide every address in the HTML.
fn render(message: &Message, depth: usize, unread: &mut usize, path: &Path) -> Result<String> {
    let mut out = headers(message);

    // `attachments()` yields nested messages too, so a forward has to be
    // recognised here or it is both followed below and announced as unread.
    for part in message.attachments().filter(|part| !part.is_message()) {
        *unread += 1;
        if let Some(name) = part.attachment_name() {
            let kind = part.content_type().map_or_else(
                || "unknown type".to_owned(),
                |content| match content.subtype() {
                    Some(subtype) => format!("{}/{subtype}", content.ctype()),
                    None => content.ctype().to_owned(),
                },
            );
            out.push_str("Attachment: ");
            out.push_str(printable(name).trim());
            out.push_str(" (");
            out.push_str(&kind);
            out.push_str(")\n");
        }
    }

    let already_read: HashSet<u32> = message.text_body.iter().copied().collect();
    let bodies = message.text_body.iter().chain(
        message
            .html_body
            .iter()
            .filter(|id| !already_read.contains(id)),
    );

    for &id in bodies {
        let Some(part) = message.parts.get(id as usize) else {
            continue;
        };
        if part.is_encoding_problem {
            bail!(
                "{} has a body part whose encoding could not be decoded, so it \
                 would be read with less text than it holds",
                path.display()
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
            continue;
        }
        push_block(&mut out, "Forwarded message:");
        push_block(&mut out, &render(nested, depth + 1, unread, path)?);
    }

    Ok(out)
}

/// Flattens HTML to text, after making its block structure survivable.
fn html_text(html: &str) -> String {
    let linked = LINK_TARGET.replace_all(html, "$0 $1 ");
    let spaced = BLOCK_TAG.replace_all(&linked, "\n$0");
    mail_parser::decoders::html::html_to_text(&spaced)
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
}
