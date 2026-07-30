//! `OpenDocument` text extraction.
//!
//! An `.odt` is a zip archive whose `content.xml` holds the body and whose
//! `styles.xml` holds headers and footers. Unlike OOXML, `ODF` puts character
//! data directly inside `text:p` and `text:h` rather than wrapping it in a run
//! element, so this needs its own element rules rather than the shared run
//! reader.

use std::path::Path;

use anyhow::{Context, Result, bail};
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::{Namespace, ResolveResult};

use super::xml::{local_name, named_entity};

/// `ODF`'s text namespace, the one whose elements carry running text.
///
/// This is read namespace-first rather than by local name because the
/// distinction it draws is namespace-bearing and nothing else separates it.
/// `svg:title`, `LibreOffice`'s alt text on an image, and `text:title`, an
/// inline field, both reduce to `title` once the prefix is stripped, and only
/// one of them is running text; `dc:date` and `text:date` are the same pair.
/// Three earlier attempts at this reader matched on the local name and each
/// closed the instance in front of it while leaving that ambiguity open.
const TEXT_NS: Namespace<'static> = Namespace(b"urn:oasis:names:tc:opendocument:xmlns:text:1.0");

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
/// Character data is running text only when both of these hold: the innermost
/// open element is in [`TEXT_NS`], and the innermost open container is a
/// paragraph. Everything else is a standalone chunk, bounded so it cannot
/// concatenate onto its neighbours.
///
/// The rule is inverted deliberately. Three earlier versions of this reader
/// listed the elements that hold metadata and treated the rest as running
/// text, and each closed the shape in front of it while leaving the class
/// open: `dc:creator`, then `meta:date-string`, then `svg:title`. A producer
/// may put anything inside an annotation or a frame, so a list of names is
/// never finished. Stated the other way round, an element nobody thought of
/// costs an extra newline instead of a leak, which is the direction this tool
/// should always fail in.
///
/// Both halves are load-bearing. The namespace alone would treat
/// `text:note-citation`, a footnote's marker, as running text and glue it to
/// the paragraph it is anchored in. The container alone would treat
/// `svg:title` on a `draw:frame` as running text, since a paragraph is open
/// around the image, and that is `LibreOffice`'s alt text: bare character data
/// holding whatever the author typed.
///
/// Containers are matched on local name rather than namespace, unlike the
/// elements. A `note` in some other namespace becoming a region costs a
/// newline; it cannot leak.
///
/// The stack holds a [`Container::Paragraph`] for each open `text:p` or
/// `text:h`, and a [`Container::Region`] for each open `office:annotation`,
/// `text:note` or `office:change-info`. A `text:p` nested inside a `text:p`,
/// which `ODF` does for an annotation anchored mid-paragraph, is why this is a
/// stack rather than a flag: a flag would clear on the inner close and drop the
/// rest of the outer paragraph.
///
/// Boundaries are emitted when an element is entered or left, never per chunk
/// of character data. `quick_xml` reports an entity reference as its own event,
/// so bounding each chunk shatters `Dupont &amp; Fils` into three lines, and a
/// name broken across lines matches no denylist term and reaches the output
/// intact.
fn extract(xml: &str) -> Result<String> {
    let mut reader = NsReader::from_str(xml);
    let mut text = String::new();
    let mut containers: Vec<Container> = Vec::new();
    let mut open: Vec<bool> = Vec::new();
    let mut skipped = 0usize;
    let mut boundary = false;

    loop {
        let (resolved, event) = reader.read_resolved_event()?;
        let in_text_ns =
            matches!(resolved, ResolveResult::Bound(namespace) if namespace == TEXT_NS);

        match event {
            Event::Start(tag) => {
                let raw = tag.name();
                match local_name(raw.as_ref()) {
                    b"binary-data" | b"script" => skipped += 1,
                    b"p" | b"h" => containers.push(Container::Paragraph),
                    b"annotation" | b"note" | b"change-info" => containers.push(Container::Region),
                    b"tab" if skipped == 0 => text.push('\t'),
                    b"line-break" if skipped == 0 => text.push('\n'),
                    _ => {}
                }
                let inline = in_text_ns && containers.last() == Some(&Container::Paragraph);
                open.push(inline);
                boundary |= !inline;
            }
            Event::Empty(tag) if skipped == 0 => match local_name(tag.name().as_ref()) {
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
                // A close with no open, in malformed XML, is treated as
                // bounded: an extra newline rather than a run-on.
                boundary |= !open.pop().unwrap_or(false);
                let raw = tag.name();
                match local_name(raw.as_ref()) {
                    b"binary-data" | b"script" => skipped = skipped.saturating_sub(1),
                    b"p" | b"h" => {
                        containers.pop();
                        end_line(&mut text);
                        boundary = false;
                    }
                    b"annotation" | b"note" | b"change-info" => {
                        containers.pop();
                    }
                    _ => {}
                }
            }
            Event::Text(chunk) if skipped == 0 => {
                push_chunk(
                    &mut text,
                    &chunk.decode()?,
                    &mut boundary,
                    inline_now(&open),
                );
            }
            Event::CData(chunk) if skipped == 0 => {
                push_chunk(
                    &mut text,
                    &chunk.decode()?,
                    &mut boundary,
                    inline_now(&open),
                );
            }
            Event::GeneralRef(reference) if skipped == 0 => {
                let inline = inline_now(&open);
                match reference.resolve_char_ref()? {
                    Some(character) => {
                        let mut buffer = [0u8; 4];
                        let encoded = character.encode_utf8(&mut buffer);
                        push_chunk(&mut text, encoded, &mut boundary, inline);
                    }
                    None => push_chunk(
                        &mut text,
                        named_entity(reference.as_ref())?,
                        &mut boundary,
                        inline,
                    ),
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(text)
}

/// Whether character data arriving now belongs to a paragraph's running text.
///
/// Character data with no element open at all, which well-formed XML does not
/// produce, is not inline: there is nothing for it to run into.
fn inline_now(open: &[bool]) -> bool {
    open.last().copied().unwrap_or(false)
}

/// Ends the current line, unless the text is empty or already ends one.
fn end_line(text: &mut String) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
}

/// Appends a piece of character data, honouring a pending boundary.
///
/// A boundary is consumed rather than emitted per chunk, which is what keeps
/// text whole across the separate events `quick_xml` reports for entity
/// references.
///
/// A chunk that is only whitespace is dropped when it is not running text, and
/// leaves the boundary pending. Every element a producer indents contributes
/// one of these, and they would otherwise be the bulk of the output.
fn push_chunk(text: &mut String, chunk: &str, boundary: &mut bool, inline: bool) {
    if !inline && chunk.trim().is_empty() {
        return;
    }
    if *boundary {
        end_line(text);
        *boundary = false;
    }
    text.push_str(chunk);
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
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"><office:body><office:text>{inner}</office:text></office:body></office:document-content>"#
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

    /// `svg:title` and `svg:desc` on a `draw:frame` are `LibreOffice`'s alt
    /// text, written whenever an author sets it on an image. They are bare
    /// character data sitting inside the paragraph the image is anchored in,
    /// so a rule that asks only whether a paragraph is open treats them as
    /// running text and glues them to their neighbours.
    #[test]
    fn image_alt_text_does_not_glue_to_the_paragraph_it_sits_in() {
        let xml = content(
            "<text:p>Signe par<draw:frame><svg:title>Jean Dupont</svg:title><svg:desc>photo prise a Lille</svg:desc><draw:image/></draw:frame> suite</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            !text.contains("parJean Dupont"),
            "the alt text glued onto the host paragraph:\n{text}"
        );
        assert!(
            text.contains("Jean Dupont"),
            "the alt text was dropped entirely:\n{text}"
        );
    }

    /// The namespace is what separates `svg:title` from `text:title`: both
    /// reduce to `title` once the prefix is stripped, and only one of them is
    /// running text. Matching on the local name alone cannot tell them apart,
    /// which is why no list of names ever closed this.
    #[test]
    fn an_inline_title_field_is_not_confused_with_image_alt_text() {
        let xml = content("<text:p>Objet : <text:title>Contrat</text:title> signe</text:p>");
        assert_eq!(extract(&xml).expect("parsing"), "Objet : Contrat signe\n");
    }

    /// `quick_xml` emits a separate event at every entity reference, so
    /// bounding each character-data event shatters a name across lines rather
    /// than keeping it whole. `Dupont & Fils` broken into `Dupont`, `&` and
    /// `Fils` matches no denylist term and reaches the output unredacted.
    #[test]
    fn an_entity_reference_does_not_shatter_bounded_text() {
        let xml = content(
            "<office:annotation><dc:creator>Dupont &amp; Fils</dc:creator><text:p>a note</text:p></office:annotation>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            text.contains("Dupont & Fils"),
            "the creator was shattered by the entity reference:\n{text}"
        );
    }

    /// The same failure with a numeric character reference, which is how an
    /// accented name arrives.
    #[test]
    fn a_numeric_character_reference_does_not_shatter_bounded_text() {
        let xml = content(
            "<office:annotation><dc:creator>Jos&#233;phine Dupont</dc:creator><text:p>a note</text:p></office:annotation>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            text.contains("Joséphine Dupont"),
            "the creator was shattered by the character reference:\n{text}"
        );
    }

    /// An image's bytes are not text anyone wrote. Running base64 through
    /// every detector is wasted work at best, and a long digit run inside it
    /// is a candidate for the identifier patterns at worst.
    #[test]
    fn binary_data_does_not_reach_the_text() {
        let xml = content(
            "<text:p>Photo<draw:frame><draw:image><office:binary-data>iVBORw0KGgoAAAANSUhEUg</office:binary-data></draw:image></draw:frame></text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            !text.contains("iVBORw0KGgo"),
            "base64 image data reached the extracted text:\n{text}"
        );
    }

    /// Script source is not text anyone wrote either, and a macro can be
    /// long enough to dominate a small document.
    #[test]
    fn script_source_does_not_reach_the_text() {
        let xml = content(
            "<office:script>Sub Main\n  MsgBox \"0612345678\"\nEnd Sub</office:script><text:p>Jean Dupont</text:p>",
        );
        let text = extract(&xml).expect("parsing");
        assert!(
            !text.contains("MsgBox"),
            "script source reached the extracted text:\n{text}"
        );
        assert!(
            text.contains("Jean Dupont"),
            "the body was dropped:\n{text}"
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
