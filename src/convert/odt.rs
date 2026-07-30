//! `OpenDocument` text extraction.
//!
//! An `.odt` is a zip archive whose `content.xml` holds the body and whose
//! `styles.xml` holds headers and footers. Unlike OOXML, `ODF` puts character
//! data directly inside `text:p` and `text:h` rather than wrapping it in a run
//! element, so this needs its own element rules rather than the shared run
//! reader.

use std::path::Path;

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;

use super::xml::{local_name, named_entity};

/// The largest run of spaces a `text:s` will expand to.
///
/// `text:c` is a document-supplied count with no upper bound of its own, and
/// nothing stops it naming billions of spaces from a handful of bytes. Real
/// documents use `text:s` for indentation and alignment, not for megabytes of
/// whitespace, so a few thousand is ample; a count above this is treated as
/// malformed rather than honoured.
const SPACE_RUN_LIMIT: usize = 4096;

/// The archive member holding the body. Also the marker for a readable file.
const CONTENT: &str = "content.xml";
/// Headers and footers live here, not in the body, so a letterhead or a
/// contact line would be missing from a document read without it.
const STYLES: &str = "styles.xml";

/// Reads the body and the styles, in that order.
///
/// # Errors
///
/// Returns an error if the file is not a readable archive, if it holds no
/// `content.xml`, if a part cannot be read or parsed, or if it yields no text.
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

/// A container `ODF` character data can be nested inside.
///
/// The distinction is what decides whether that character data is part of
/// a paragraph's running text or a standalone chunk; see [`extract`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    /// `text:p` or `text:h`.
    Paragraph,
    /// `office:annotation`, `text:note` or `office:change-info`: elements
    /// that hold metadata as bare character data, such as `dc:creator` and
    /// `dc:date`, sitting alongside a nested paragraph rather than only
    /// holding one.
    Region,
}

/// Pulls the text out of an `ODF` part.
///
/// Whether character data belongs to a paragraph's running text is decided
/// by position, tracked with a stack of open containers, rather than by
/// enumerating the names of elements that hold metadata. `ODF` puts the
/// metadata a real producer writes as an annotation's first children,
/// `dc:creator` and `dc:date`, directly inside it with no `text:p` of its
/// own, and a footnote's `text:note-citation` is the same shape; naming
/// those was previously how this reader told them apart from running text.
/// That approach cannot close the class it is trying to describe: `ODF`
/// also defines `meta:date-string`, the localised rendering of `dc:date`, as
/// a further child of `office:annotation` with the same shape, and any
/// element this reader has not been told about is missed the same way.
///
/// The stack holds a [`Container::Paragraph`] for each open `text:p` or
/// `text:h`, and a [`Container::Region`] for each open `office:annotation`,
/// `text:note` or `office:change-info`, the elements that hold metadata
/// alongside a nested paragraph rather than only holding one. Character
/// data is inline, appended straight into the running text, only when the
/// innermost open container is a paragraph; otherwise it is a standalone
/// chunk, bounded by newlines through [`push_chunk`]. Position rather than
/// name is also why `text:date` and `text:creator`, `ODF`'s inline fields,
/// no longer need special handling: nested inside an ordinary paragraph,
/// they inherit that paragraph's own inline treatment without being pushed
/// onto the stack at all.
///
/// A `text:p` nested inside a `text:p`, which `ODF` does for annotations and
/// footnotes anchored mid-paragraph, is why the paragraph state is a stack
/// rather than a flag: a flag would clear on the inner close and drop the
/// rest of the outer paragraph.
fn extract(xml: &str) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut text = String::new();
    let mut stack: Vec<Container> = Vec::new();

    loop {
        match reader.read_event()? {
            Event::Start(tag) => match local_name(tag.name().as_ref()) {
                b"p" | b"h" => stack.push(Container::Paragraph),
                b"annotation" | b"note" | b"change-info" => stack.push(Container::Region),
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
            Event::End(tag) => match local_name(tag.name().as_ref()) {
                b"p" | b"h" => {
                    stack.pop();
                    text.push('\n');
                }
                b"annotation" | b"note" | b"change-info" => {
                    stack.pop();
                }
                _ => {}
            },
            Event::Text(chunk) => {
                push_chunk(&mut text, &chunk.decode()?, inline(&stack));
            }
            Event::CData(chunk) => {
                push_chunk(&mut text, &chunk.decode()?, inline(&stack));
            }
            Event::GeneralRef(reference) => match reference.resolve_char_ref()? {
                Some(character) => {
                    let mut buffer = [0u8; 4];
                    push_chunk(
                        &mut text,
                        character.encode_utf8(&mut buffer),
                        inline(&stack),
                    );
                }
                None => push_chunk(&mut text, named_entity(reference.as_ref())?, inline(&stack)),
            },
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(text)
}

/// Whether the innermost open container is a paragraph, so character data
/// here belongs to its running text rather than to a standalone chunk.
///
/// An empty stack, character data before the first paragraph or after the
/// last, is not inline: there is no paragraph for it to run into.
fn inline(stack: &[Container]) -> bool {
    matches!(stack.last(), Some(Container::Paragraph))
}

/// Appends a piece of character data: straight into the running text when
/// `inline`, otherwise as a standalone chunk bounded by newlines.
///
/// A chunk that is only whitespace is dropped when not inline. Capture is no
/// longer gated on paragraph depth, so the insignificant whitespace a
/// producer writes between elements, for indentation, would otherwise
/// become noise in the output.
fn push_chunk(text: &mut String, chunk: &str, inline: bool) {
    if inline {
        text.push_str(chunk);
        return;
    }
    if chunk.trim().is_empty() {
        return;
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(chunk);
    text.push('\n');
}

/// How many spaces a `text:s` stands for.
///
/// The `text:c` attribute is optional and defaults to one. A malformed count
/// is treated as one rather than failing: it costs a space in the layout the
/// detectors see, where failing would refuse an otherwise readable document.
/// A count above [`SPACE_RUN_LIMIT`] is treated the same way, as malformed,
/// rather than clamped to the ceiling: clamping would still honour an
/// obviously bogus value by inventing layout the document never had.
fn space_count(tag: &quick_xml::events::BytesStart<'_>) -> Result<usize> {
    for attribute in tag.attributes() {
        let attribute = attribute?;
        if local_name(attribute.key.as_ref()) == b"c" {
            let count = std::str::from_utf8(&attribute.value)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|count| *count <= SPACE_RUN_LIMIT)
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

    /// A real producer writes `dc:creator` and `dc:date` as the first
    /// children of `office:annotation`, both character data with no
    /// `text:p` of their own. Without a boundary at the annotation itself,
    /// the commenter's name concatenates onto the preceding word with no
    /// separator, which is how it used to escape redaction.
    #[test]
    fn an_annotations_author_name_does_not_glue_to_the_host_paragraph() {
        let xml = content(
            "<text:p>Signe par<office:annotation><dc:creator>Jean Dupont</dc:creator><dc:date>2026-07-30T08:15:29</dc:date><text:p>rappeler</text:p></office:annotation> suite</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            !text.contains("parJean"),
            "the author's name glued onto the host paragraph:\n{text}"
        );
        assert!(
            text.contains("Jean Dupont"),
            "the author's name was dropped entirely:\n{text}"
        );
    }

    /// `text:note-citation` glues onto the host paragraph the same way
    /// `dc:creator` does: it is character data sitting directly inside
    /// `text:note`, itself inline in the paragraph it footnotes, with no
    /// `text:p` of its own.
    #[test]
    fn a_footnote_citation_does_not_glue_to_the_host_paragraph() {
        let xml = content(
            "<text:p>Signe par<text:note><text:note-citation>1</text:note-citation><text:note-body><text:p>Jean Dupont</text:p></text:note-body></text:note> suite</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            !text.contains("par1"),
            "the citation glued onto the host paragraph:\n{text}"
        );
    }

    /// `meta:date-string` is a spec-defined child of `office:annotation`
    /// (`ODF` 1.2/1.3 Part 1 section 14.3), the localised rendering of
    /// `dc:date`, and is character data with no `text:p` of its own, the
    /// same shape as `dc:creator` and `dc:date`. Enumerating names cannot
    /// close this class, since a producer may write metadata this reader has
    /// never heard of; only position closes it.
    #[test]
    fn an_annotations_date_string_does_not_glue_to_its_nested_paragraph() {
        let xml = content(
            "<text:p>Signe par<office:annotation><dc:creator>Jean Dupont</dc:creator><dc:date>2026-07-30T08:15:29</dc:date><meta:date-string>30/07/2026 08:15</meta:date-string><text:p>Jean Dupont doit rappeler</text:p></office:annotation> suite</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            !text.contains("08:15Jean Dupont doit rappeler"),
            "the date-string glued onto the nested paragraph:\n{text}"
        );
    }

    /// Matching a boundary on local name alone, rather than on position,
    /// also caught `ODF`'s inline fields `text:date` and `text:creator`,
    /// which sit inside an ordinary paragraph rather than beside a nested
    /// one. That is a fidelity loss, not a leak: a signature line reads as
    /// five lines instead of one.
    #[test]
    fn inline_date_and_creator_fields_stay_on_one_line() {
        let xml = content(
            "<text:p>Fait a Lille le <text:date>30/07/2026</text:date> par <text:creator>Jean Dupont</text:creator>, tel 06 12 34 56 78</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert_eq!(
            text,
            "Fait a Lille le 30/07/2026 par Jean Dupont, tel 06 12 34 56 78\n"
        );
    }

    /// Gating capture on paragraph depth drops character data that sits
    /// outside any paragraph entirely, with nothing reported: an annotation
    /// anchored directly on a table cell, and an `office:change-info` inside
    /// `text:tracked-changes`, both real shapes a producer writes. Losing
    /// this silently breaks the rule in `src/convert/mod.rs` that a
    /// conversion produces the document's real text or fails.
    #[test]
    fn character_data_outside_any_paragraph_is_still_read() {
        let xml = content(
            "<table:table-cell><office:annotation><dc:creator>Jean Dupont</dc:creator><dc:date>2026-07-30T08:15:29</dc:date></office:annotation></table:table-cell><text:tracked-changes><text:changed-region><text:insertion><office:change-info><dc:creator>Marie Martin</dc:creator><dc:date>2026-07-30T08:20:00</dc:date></office:change-info></text:insertion></text:changed-region></text:tracked-changes>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            text.contains("Jean Dupont"),
            "the annotation's creator, anchored on a table cell with no paragraph, was dropped:\n{text}"
        );
        assert!(
            text.contains("Marie Martin"),
            "the tracked change's creator was dropped:\n{text}"
        );
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

    /// A `text:c` far past what any real document needs must not be honoured
    /// verbatim: doing so would let a handful of bytes expand to gigabytes of
    /// spaces. Without the ceiling this hangs rather than fails, so this test
    /// completing at all is the coverage.
    #[test]
    fn a_space_count_far_above_the_ceiling_is_treated_as_malformed() {
        let xml = content("<text:p>a<text:s text:c=\"9999999999\"/>b</text:p>");
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

    /// Character data in a `CDATA` section must reach the output the same as
    /// ordinary text, rather than being silently dropped.
    #[test]
    fn cdata_inside_a_paragraph_is_kept() {
        let xml = content("<text:p><![CDATA[Jean Dupont]]></text:p>");
        assert_eq!(extract(&xml).expect("parsing"), "Jean Dupont\n");
    }

    #[test]
    fn a_file_that_is_not_a_zip_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.odt");
        std::fs::write(&path, "this is not a zip").expect("writing");
        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable .odt"));
    }

    /// An archive holding `styles.xml` but no `content.xml` is not a readable
    /// `.odt`, and the message must name the missing part rather than claim
    /// the whole archive is empty.
    #[test]
    fn a_missing_content_part_is_refused_by_name() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("no-content.odt");
        let file = std::fs::File::create(&path).expect("creating");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file(STYLES, options).expect("starting entry");
        writer
            .write_all(content("<text:p>a</text:p>").as_bytes())
            .expect("writing entry");
        writer.finish().expect("finishing archive");

        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains(CONTENT));
    }

    /// Not every producer writes `styles.xml`; its absence is tolerated
    /// rather than refused, so the body must still come back.
    #[test]
    fn a_missing_styles_part_is_tolerated() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("no-styles.odt");
        let file = std::fs::File::create(&path).expect("creating");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file(CONTENT, options).expect("starting entry");
        writer
            .write_all(content("<text:p>Jean Dupont</text:p>").as_bytes())
            .expect("writing entry");
        writer.finish().expect("finishing archive");

        let text = to_text(&path).expect("reading");
        assert!(text.contains("Jean Dupont"), "body dropped:\n{text}");
    }
}
