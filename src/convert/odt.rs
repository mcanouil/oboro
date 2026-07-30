//! `OpenDocument` text extraction.
//!
//! An `.odt` is a zip archive whose `content.xml` holds the body and whose
//! `styles.xml` holds headers and footers. Unlike OOXML, ODF puts character
//! data directly inside `text:p` and `text:h` rather than wrapping it in a run
//! element, so this needs its own element rules rather than the shared run
//! reader.
//!
//! Nothing calls [`to_text`] yet: the dispatch that reaches it belongs to a
//! later step of the pptx/odt reader work, which is why the module is
//! otherwise complete but unreachable. Each item below carries its own
//! `#[allow(dead_code)]` for that reason; remove all five once a later step
//! adds the `Format::Odt` dispatch arm that calls [`to_text`].

use std::path::Path;

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;

use super::xml::{local_name, named_entity};

/// The archive member holding the body. Also the marker for a readable file.
#[allow(dead_code)]
const CONTENT: &str = "content.xml";
/// Headers and footers live here, not in the body, so a letterhead or a
/// contact line would be missing from a document read without it.
#[allow(dead_code)]
const STYLES: &str = "styles.xml";

/// Reads the body and the styles, in that order.
///
/// # Errors
///
/// Returns an error if the file is not a readable archive, if it holds no
/// `content.xml`, if a part cannot be read or parsed, or if it yields no text.
#[allow(dead_code)]
pub fn to_text(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable .odt archive", path.display()))?;

    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    if !names.iter().any(|name| name == CONTENT) {
        bail!(
            "{} has no {CONTENT}; it may not be an OpenDocument file",
            path.display()
        );
    }

    let mut text = String::new();
    for part in [CONTENT, STYLES] {
        if !names.iter().any(|name| name == part) {
            continue;
        }
        let xml = super::xml::read_part(&mut archive, part, path)?;
        let extracted =
            extract(&xml).with_context(|| format!("parsing {part} of {}", path.display()))?;
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

/// Pulls the text out of an ODF part.
///
/// Paragraph tracking is a depth counter rather than a flag because ODF nests
/// a `text:p` inside a `text:p` for annotations and footnotes, and a flag
/// would clear on the inner close and drop the rest of the outer paragraph.
#[allow(dead_code)]
fn extract(xml: &str) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut text = String::new();
    let mut depth = 0usize;

    loop {
        match reader.read_event()? {
            Event::Start(tag) => match local_name(tag.name().as_ref()) {
                b"p" | b"h" => depth += 1,
                b"tab" => text.push('\t'),
                b"line-break" => text.push('\n'),
                _ => {}
            },
            Event::Empty(tag) => match local_name(tag.name().as_ref()) {
                b"tab" => text.push('\t'),
                b"line-break" => text.push('\n'),
                b"s" => {
                    for _ in 0..space_count(&tag)? {
                        text.push(' ');
                    }
                }
                _ => {}
            },
            Event::End(tag) => {
                if matches!(local_name(tag.name().as_ref()), b"p" | b"h") {
                    depth = depth.saturating_sub(1);
                    text.push('\n');
                }
            }
            Event::Text(chunk) if depth > 0 => text.push_str(&chunk.decode()?),
            Event::GeneralRef(reference) if depth > 0 => match reference.resolve_char_ref()? {
                Some(character) => text.push(character),
                None => text.push_str(named_entity(reference.as_ref())?),
            },
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(text)
}

/// How many spaces a `text:s` stands for.
///
/// The `text:c` attribute is optional and defaults to one. A malformed count
/// is treated as one rather than failing: it costs a space in the layout the
/// detectors see, where failing would refuse an otherwise readable document.
#[allow(dead_code)]
fn space_count(tag: &quick_xml::events::BytesStart<'_>) -> Result<usize> {
    for attribute in tag.attributes() {
        let attribute = attribute?;
        if local_name(attribute.key.as_ref()) == b"c" {
            let count = std::str::from_utf8(&attribute.value)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            return Ok(count);
        }
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:text>{inner}</office:text></office:body></office:document-content>"#
        )
    }

    #[test]
    fn reads_paragraphs_and_headings() {
        let xml = content("<text:h>Acme Consulting SARL</text:h><text:p>Jean Dupont</text:p>");
        assert_eq!(
            extract(&xml).expect("parsing"),
            "Acme Consulting SARL\nJean Dupont\n"
        );
    }

    #[test]
    fn separates_paragraphs_so_they_do_not_run_together() {
        let xml = content("<text:p>0612345678</text:p><text:p>9876</text:p>");
        let text = extract(&xml).expect("parsing");
        assert_eq!(text, "0612345678\n9876\n");
        assert!(!text.contains("06123456789876"));
    }

    /// An annotation anchored mid-paragraph nests a `text:p` inside a
    /// `text:p`. A boolean in-paragraph flag would clear on the inner close
    /// and drop the rest of the outer paragraph.
    #[test]
    fn an_annotation_does_not_truncate_the_paragraph_holding_it() {
        let xml = content(
            "<text:p>12 bis rue de la Paix<office:annotation><text:p>a note</text:p></office:annotation>, 75002 Paris</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            text.contains("75002 Paris"),
            "text after the annotation was dropped:\n{text}"
        );
        assert!(text.contains("a note"), "annotation text dropped:\n{text}");
    }

    #[test]
    fn a_space_element_expands_its_count() {
        let xml = content("<text:p>a<text:s text:c=\"3\"/>b</text:p>");
        assert_eq!(extract(&xml).expect("parsing"), "a   b\n");
    }

    #[test]
    fn a_space_element_without_a_count_is_one_space() {
        let xml = content("<text:p>a<text:s/>b</text:p>");
        assert_eq!(extract(&xml).expect("parsing"), "a b\n");
    }

    #[test]
    fn tabs_and_line_breaks_are_kept() {
        let xml = content("<text:p>a<text:tab/>b<text:line-break/>c</text:p>");
        assert_eq!(extract(&xml).expect("parsing"), "a\tb\nc\n");
    }

    #[test]
    fn decodes_entities_and_accents() {
        let xml = content("<text:p>Soci&#233;t&#233; &amp; Fils</text:p>");
        assert_eq!(extract(&xml).expect("parsing"), "Société & Fils\n");
    }

    #[test]
    fn a_file_that_is_not_a_zip_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.odt");
        std::fs::write(&path, "this is not a zip").expect("writing");
        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable .odt"));
    }
}
