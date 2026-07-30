//! XML reading shared by the readers for zip-and-XML document formats.
//!
//! `.docx`, `.pptx` and `.odt` are all zip archives of XML parts, and all
//! three need the same three things: a part read into a string, namespace
//! prefixes stripped so `w:t` and `a:t` match one rule, and the five entities
//! XML predefines expanded.
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

/// Reads one archive member into a string.
///
/// The part name comes from the archive, so it is the document's to choose and
/// is bounded by [`super::quoted`] before it reaches a message.
pub(super) fn read_part<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    part: &str,
    document: &Path,
) -> Result<String> {
    let named = super::quoted(part);
    let mut xml = String::new();
    archive
        .by_name(part)
        .with_context(|| format!("opening {named} of {}", document.display()))?
        .read_to_string(&mut xml)
        .with_context(|| format!("reading {named} of {}", document.display()))?;
    Ok(xml)
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
/// `br` is matched as both `Start` and `Empty`. `WordprocessingML`'s break is
/// childless and so always self-closing, but `DrawingML`'s takes an optional
/// `a:rPr` child, and a dropped break merges the runs either side.
pub(super) fn runs(xml: &str, text_elements: &[&[u8]]) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut text = String::new();
    let mut depth = 0usize;

    loop {
        match reader.read_event()? {
            Event::Start(tag) => {
                let raw = tag.name();
                let name = local_name(raw.as_ref());
                if text_elements.contains(&name) {
                    depth += 1;
                } else if name == b"tab" {
                    text.push('\t');
                } else if name == b"br" || name == b"cr" {
                    text.push('\n');
                }
            }
            Event::Empty(tag) => match local_name(tag.name().as_ref()) {
                b"br" | b"cr" => text.push('\n'),
                b"tab" => text.push('\t'),
                _ => {}
            },
            Event::End(tag) => {
                let raw = tag.name();
                let name = local_name(raw.as_ref());
                if text_elements.contains(&name) {
                    depth = depth.saturating_sub(1);
                } else if name == b"p" {
                    text.push('\n');
                }
            }
            Event::Text(chunk) if depth > 0 => {
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
        let text = runs(&xml, &[b"t"]).expect("parsing");
        assert_eq!(text, "75002\n0612345678\n");
        assert!(
            !text.contains("750020612345678"),
            "a break with run properties must not merge its neighbours"
        );
    }
}
