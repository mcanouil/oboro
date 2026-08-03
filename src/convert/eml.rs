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

use anyhow::{Context, Result, bail};
use mail_parser::{Address, Message, MessageParser};

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

    let text = render(&message);

    if text.trim().is_empty() {
        bail!(
            "{} contains no extractable text; if its content is attachments, \
             read those separately",
            path.display()
        );
    }
    Ok(text)
}

/// Renders one message: headers, then bodies.
fn render(message: &Message) -> String {
    let mut out = headers(message);
    for &id in &message.text_body {
        if let Some(part) = message.parts.get(id as usize)
            && let mail_parser::PartType::Text(text) = &part.body
        {
            push_block(&mut out, text);
        }
    }
    out
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
}
