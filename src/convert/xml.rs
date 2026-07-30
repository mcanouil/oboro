//! XML reading shared by the readers for zip-and-XML document formats.
//!
//! `.docx` and `.pptx` are both zip archives of XML parts, and both need the
//! same three things: a part read into a string, namespace prefixes stripped
//! so `w:t` and `a:t` match one rule, and the five entities XML predefines
//! expanded.
//!
//! Entity expansion is the reason this is shared rather than copied. Dropping
//! an entity reference silently strips accents, so "Société" would reach the
//! detectors as "Socit" and no longer match a denylisted name. A second copy
//! of that rule is a second place for it to be got wrong.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;

/// Strips the namespace prefix, so `w:t` and a bare `t` both match.
pub(super) fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|byte| *byte == b':') {
        Some(colon) => &name[colon + 1..],
        None => name,
    }
}

/// Expands the five entities XML predefines.
///
/// Anything else is a document-defined entity this reader does not carry a
/// declaration for; failing is better than dropping characters from a
/// document the user is about to share.
pub(super) fn named_entity(name: &[u8]) -> Result<&'static str> {
    match name {
        b"amp" => Ok("&"),
        b"lt" => Ok("<"),
        b"gt" => Ok(">"),
        b"quot" => Ok("\""),
        b"apos" => Ok("'"),
        other => bail!(
            "the document uses an entity '&{};' this reader cannot expand",
            super::quoted(&String::from_utf8_lossy(other))
        ),
    }
}

/// The most a single archive part may decompress to.
///
/// These parts are read as the archive is opened, before any of the size
/// checks that run once a document is assembled into text apply. Deflate
/// reaches roughly 1000:1 on a repeated byte, and a run of `A` is valid
/// UTF-8, so a small, malicious `.docx` or `.pptx` could otherwise inflate
/// one part to gigabytes before this code has even started parsing it, and
/// `pptx.rs` then accumulates every part into one more string on top of
/// that. A single XML part is much smaller than a whole
/// document, so this sits well under the PDF reader's 256 MiB ceiling on a
/// whole decompressed cross-reference stream; 64 MiB is comfortably past the
/// largest part a real document has been seen to carry.
pub(super) const MAX_PART_BYTES: usize = 64 * 1024 * 1024;

/// Reads one archive member into a string.
///
/// The part name comes from the archive, so it is the document's to choose and
/// is bounded by [`super::quoted`] before it reaches a message.
///
/// The read is capped at [`MAX_PART_BYTES`] plus one byte: `take` alone would
/// silently truncate an oversized part and hand back half a document, which
/// is exactly what this reader must never do, so the extra byte is checked
/// and the part refused by name rather than read short.
pub(super) fn read_part<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    part: &str,
    document: &Path,
) -> Result<String> {
    let named = super::quoted(part);
    let mut bytes = Vec::new();
    archive
        .by_name(part)
        .with_context(|| format!("opening {named} of {}", document.display()))?
        .take(MAX_PART_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {named} of {}", document.display()))?;
    if bytes.len() > MAX_PART_BYTES {
        bail!(
            "{named} of {} decompresses past {MAX_PART_BYTES} bytes; refusing to read it",
            document.display()
        );
    }
    String::from_utf8(bytes)
        .with_context(|| format!("{named} of {} is not valid UTF-8", document.display()))
}

/// Pulls the text runs out of an OOXML part.
///
/// Paragraphs and line breaks become newlines and tabs become tabs, so the
/// layout the detectors see resembles the document a reader sees. Without
/// that, adjacent paragraphs would run together and invent entities that
/// span a boundary.
///
/// `text_elements` names the local elements whose character data is text.
/// It is a parameter rather than a constant because the legacy `PowerPoint`
/// comment part puts its text in `p:text` instead of a run, and widening the
/// rule for every part of every format would make any element named `text`
/// text-bearing.
///
/// `line_terminators` names the local elements whose *close* ends a line.
/// It is a separate parameter, rather than folded into `text_elements`,
/// because the two roles do not always land on the same element: an ordinary
/// paragraph both holds text and ends a line, but the legacy comment format's
/// `p:text` holds an entire comment with no paragraph wrapping it at all, so
/// closing it has to end a line too or two consecutive comments in the same
/// part run together with nothing to tell where one ends and the next
/// begins.
///
/// `br` is matched as both `Start` and `Empty`. `WordprocessingML`'s break is
/// childless and so always self-closing, but `DrawingML`'s takes an optional
/// `a:rPr` child, and a dropped break merges the runs either side.
///
/// A `tab` inside a `tabs` or `tabLst` container is not a tab in the text: it
/// is a tab-stop *definition*, part of paragraph formatting rather than
/// character data, and would otherwise inject a stray leading tab into the
/// paragraph that follows.
pub(super) fn runs(
    xml: &str,
    text_elements: &[&[u8]],
    line_terminators: &[&[u8]],
) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut text = String::new();
    let mut depth = 0usize;
    let mut tab_definitions = 0usize;

    loop {
        match reader.read_event()? {
            Event::Start(tag) => {
                let raw = tag.name();
                let name = local_name(raw.as_ref());
                if text_elements.contains(&name) {
                    depth += 1;
                } else if name == b"tabs" || name == b"tabLst" {
                    tab_definitions += 1;
                } else if name == b"tab" && tab_definitions == 0 {
                    text.push('\t');
                } else if name == b"br" || name == b"cr" {
                    text.push('\n');
                }
            }
            Event::Empty(tag) => match local_name(tag.name().as_ref()) {
                b"br" | b"cr" => text.push('\n'),
                b"tab" if tab_definitions == 0 => text.push('\t'),
                _ => {}
            },
            Event::End(tag) => {
                let raw = tag.name();
                let name = local_name(raw.as_ref());
                if text_elements.contains(&name) {
                    depth = depth.saturating_sub(1);
                }
                if line_terminators.contains(&name) {
                    text.push('\n');
                }
                if name == b"tabs" || name == b"tabLst" {
                    tab_definitions = tab_definitions.saturating_sub(1);
                }
            }
            Event::Text(chunk) if depth > 0 => {
                text.push_str(&chunk.decode()?);
            }
            Event::CData(chunk) if depth > 0 => {
                text.push_str(&chunk.decode()?);
            }
            // Entity references arrive as their own events, so ignoring them
            // would quietly drop every accent.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<a:txBody xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">{inner}</a:txBody>"#
        )
    }

    /// `DrawingML`'s line break takes an optional `a:rPr` child, so the element
    /// can arrive as a `Start`/`End` pair rather than as `Empty`. Dropping it
    /// would merge the runs either side and invent an entity spanning them.
    #[test]
    fn a_break_carrying_run_properties_still_separates_runs() {
        let xml = wrap(
            "<a:p><a:r><a:t>75002</a:t></a:r><a:br><a:rPr/></a:br><a:r><a:t>0612345678</a:t></a:r></a:p>",
        );
        let text = runs(&xml, &[b"t"], &[b"p"]).expect("parsing");
        assert_eq!(text, "75002\n0612345678\n");
        assert!(
            !text.contains("750020612345678"),
            "a break with run properties must not merge its neighbours"
        );
    }

    /// A `w:tabs`/`w:tab` block defines tab *stops* inside `w:pPr`; it carries
    /// no character data of its own and must not inject a tab into the text.
    #[test]
    fn a_tab_stop_definition_is_not_read_as_a_tab_character() {
        let xml = wrap(
            "<a:p><a:pPr><a:tabLst><a:tab a:pos=\"720\"/></a:tabLst></a:pPr><a:r><a:t>Title</a:t></a:r></a:p>",
        );
        let text = runs(&xml, &[b"t"], &[b"p"]).expect("parsing");
        assert_eq!(text, "Title\n");
    }

    /// Character data in a `CDATA` section must reach the output the same as
    /// ordinary text, rather than being silently dropped.
    #[test]
    fn cdata_inside_a_run_is_kept() {
        let xml = wrap("<a:p><a:r><a:t><![CDATA[Jean Dupont]]></a:t></a:r></a:p>");
        let text = runs(&xml, &[b"t"], &[b"p"]).expect("parsing");
        assert_eq!(text, "Jean Dupont\n");
    }

    /// A part that decompresses past the ceiling is refused by name rather
    /// than read into an unbounded string. `deflate` reaches roughly 1000:1
    /// on repeated bytes, and a run of `A` is valid UTF-8, so an innocuous
    /// looking archive could otherwise inflate one part to gigabytes before
    /// this code even starts parsing it.
    #[test]
    fn a_part_past_the_decompression_ceiling_is_refused_by_name() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("bomb.pptx");
        let file = std::fs::File::create(&path).expect("creating");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file("content.xml", options)
            .expect("starting entry");
        let oversized = vec![b'A'; MAX_PART_BYTES + 1];
        writer.write_all(&oversized).expect("writing entry");
        writer.finish().expect("finishing archive");

        let file = std::fs::File::open(&path).expect("opening");
        let mut archive = zip::ZipArchive::new(file).expect("reading archive");
        let error =
            read_part(&mut archive, "content.xml", &path).expect_err("must refuse the part");
        assert!(
            format!("{error:#}").contains("content.xml"),
            "the error must name the offending part"
        );
    }
}
