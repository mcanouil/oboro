//! Word document text extraction.
//!
//! A `.docx` is a zip archive whose `word/document.xml` holds the body. Only
//! the text runs are needed, so the file is streamed rather than parsed into
//! a document model: less code, no heavyweight dependency, and nothing to go
//! wrong in the parts that are not text.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// The archive member holding the document body.
const BODY: &str = "word/document.xml";

pub fn to_text(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable .docx archive", path.display()))?;

    let parts = text_parts(&archive);
    if !parts.iter().any(|part| part.eq_ignore_ascii_case(BODY)) {
        bail!(
            "{} has no {BODY}; it may be an older .doc renamed to .docx",
            path.display()
        );
    }

    let mut text = String::new();
    for part in parts {
        let xml = super::xml::read_part(&mut archive, &part, path)?;
        let extracted = super::xml::runs(&xml, &[b"t"], &[b"p"])
            .with_context(|| format!("parsing {} of {}", super::quoted(&part), path.display()))?;
        text.push_str(&extracted);
    }

    if text.trim().is_empty() {
        bail!(
            "{} contains no extractable text; if its content is images, read those separately",
            path.display()
        );
    }
    Ok(text)
}

/// The archive members that carry readable text, in reading order.
///
/// A letterhead lives in a header part and a contact line often in a footer,
/// so reading only the body would hand back a document missing exactly the
/// details worth redacting. Comments and footnotes are included for the same
/// reason: they are text the author wrote and may well share.
fn text_parts<R: std::io::Read + std::io::Seek>(archive: &zip::ZipArchive<R>) -> Vec<String> {
    let mut headers = Vec::new();
    let mut body = Vec::new();
    let mut rest = Vec::new();

    for name in archive.file_names() {
        let Some(part) = name.strip_prefix("word/") else {
            continue;
        };
        // Producers vary in casing, and a part missed here is text silently
        // dropped from a document the user is about to share.
        let part = part.to_ascii_lowercase();
        // The lint fires on the literal comparison; `part` is already folded
        // to lowercase on the line above, so the check is case-insensitive.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        if !part.ends_with(".xml") || part.contains('/') {
            continue;
        }
        if part.starts_with("header") {
            headers.push(name.to_owned());
        } else if part == "document.xml" {
            body.push(name.to_owned());
        } else if part.starts_with("footer")
            || part == "footnotes.xml"
            || part == "endnotes.xml"
            || part == "comments.xml"
        {
            rest.push(name.to_owned());
        }
    }

    // Sorting keeps header1 before header2, so output order is stable rather
    // than following whatever order the archive happens to list.
    headers.sort();
    rest.sort();
    headers.into_iter().chain(body).chain(rest).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>{inner}</w:body></w:document>"#
        )
    }

    #[test]
    fn joins_runs_within_a_paragraph() {
        let xml = body("<w:p><w:r><w:t>Jean </w:t></w:r><w:r><w:t>Dupont</w:t></w:r></w:p>");
        assert_eq!(
            super::super::xml::runs(&xml, &[b"t"], &[b"p"]).expect("parsing"),
            "Jean Dupont\n"
        );
    }

    #[test]
    fn separates_paragraphs_so_they_do_not_run_together() {
        let xml = body(
            "<w:p><w:r><w:t>0612345678</w:t></w:r></w:p><w:p><w:r><w:t>9876</w:t></w:r></w:p>",
        );
        let text = super::super::xml::runs(&xml, &[b"t"], &[b"p"]).expect("parsing");
        assert_eq!(text, "0612345678\n9876\n");
        assert!(
            !text.contains("06123456789876"),
            "paragraphs must not merge into one number"
        );
    }

    #[test]
    fn keeps_breaks_and_tabs() {
        let xml = body("<w:p><w:r><w:t>a</w:t><w:br/><w:t>b</w:t><w:tab/><w:t>c</w:t></w:r></w:p>");
        assert_eq!(
            super::super::xml::runs(&xml, &[b"t"], &[b"p"]).expect("parsing"),
            "a\nb\tc\n"
        );
    }

    #[test]
    fn ignores_markup_outside_text_runs() {
        let xml = body(
            "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>",
        );
        assert_eq!(
            super::super::xml::runs(&xml, &[b"t"], &[b"p"]).expect("parsing"),
            "Title\n"
        );
    }

    /// A `w:tabs`/`w:tab` block inside `w:pPr` defines tab *stops*; it carries
    /// no character data of its own, so it must not read as a leading tab.
    #[test]
    fn a_tab_stop_definition_is_not_read_as_a_tab_character() {
        let xml = body(
            "<w:p><w:pPr><w:tabs><w:tab w:val=\"left\" w:pos=\"720\"/></w:tabs></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>",
        );
        assert_eq!(
            super::super::xml::runs(&xml, &[b"t"], &[b"p"]).expect("parsing"),
            "Title\n"
        );
    }

    #[test]
    fn decodes_entities_and_accents() {
        let xml = body("<w:p><w:r><w:t>Soci&#233;t&#233; &amp; Fils</w:t></w:r></w:p>");
        assert_eq!(
            super::super::xml::runs(&xml, &[b"t"], &[b"p"]).expect("parsing"),
            "Société & Fils\n"
        );
    }

    /// A letterhead lives in a header part. Reading only the body would
    /// hand back a document missing the very details worth redacting.
    #[test]
    fn reads_headers_and_footers_not_only_the_body() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("letterhead.docx");
        let text = to_text(&path).expect("reading");
        assert!(
            text.contains("Acme Consulting SARL"),
            "header dropped:\n{text}"
        );
        assert!(
            text.contains("12 bis rue de la Paix"),
            "header dropped:\n{text}"
        );
        assert!(
            text.contains("jean.dupont@acme-consulting.example"),
            "footer dropped:\n{text}"
        );
        assert!(text.contains("Corps du document"), "body dropped:\n{text}");
    }

    #[test]
    fn the_five_predefined_entities_expand() {
        assert_eq!(super::super::xml::named_entity(b"amp").expect("amp"), "&");
        assert_eq!(super::super::xml::named_entity(b"lt").expect("lt"), "<");
        assert_eq!(super::super::xml::named_entity(b"apos").expect("apos"), "'");
    }

    #[test]
    fn an_unknown_entity_fails_rather_than_dropping_characters() {
        let error =
            super::super::xml::named_entity(b"nbsp").expect_err("an undeclared entity must fail");
        assert!(
            format!("{error:#}").contains("nbsp"),
            "the error must name the offending entity"
        );
    }

    /// The entity name is the document's to choose, so it must not decide how
    /// long the error is or what reaches the terminal.
    #[test]
    fn a_hostile_entity_name_is_not_echoed_wholesale() {
        let mut hostile = b"\x1b[2J\n".to_vec();
        hostile.extend(std::iter::repeat_n(b'A', 10_000));
        let error =
            super::super::xml::named_entity(&hostile).expect_err("an undeclared entity must fail");
        let rendered = format!("{error:#}");

        assert!(
            rendered.len() < 200,
            "the message grew with the entity name: {} bytes",
            rendered.len()
        );
        assert!(
            !rendered.contains(['\x1b', '\n']),
            "control characters reached the message: {rendered:?}"
        );
    }

    #[test]
    fn a_file_that_is_not_a_zip_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.docx");
        std::fs::write(&path, "this is not a zip").expect("writing");
        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable .docx"));
    }
}
