//! `PowerPoint` presentation text extraction.
//!
//! A `.pptx` is a zip archive whose slides live under `ppt/slides/`. The text
//! is `DrawingML`, spelled `a:t` inside `a:p`, which is the same shape
//! `WordprocessingML` uses once the namespace prefix is stripped, so the shared
//! run reader handles it unchanged.
//!
//! Two producers matter and they differ. Quarto, by way of Pandoc, splits text
//! one run per word and emits separate runs holding a single space, while
//! `PowerPoint` merges a paragraph into roughly one run. The shared reader
//! concatenates run text with no separator, which is what makes the first case
//! reassemble correctly, so no separator may be introduced.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// Where the slides live. Also the marker for a readable presentation.
const SLIDES: &str = "ppt/slides/";
/// Speaker notes, read because they are text the author wrote.
const NOTES: &str = "ppt/notesSlides/";
/// Comments, read because they are author-written and carry personal data,
/// the same reason `docx.rs` reads `word/comments.xml`.
///
/// Matched by directory rather than by filename prefix: a real deck names the
/// part `modernComment_100_0.xml`, which no `comment*` prefix matches.
const COMMENTS: &str = "ppt/comments/";
/// The local elements holding comment text.
///
/// The modern format uses `DrawingML` runs, so `t` covers it. The legacy format
/// puts its text directly in `p:text`, which would otherwise read as empty.
/// Scoped to comments rather than added to the shared reader, so no element
/// named `text` becomes text-bearing across every part of every format.
const COMMENT_TEXT: &[&[u8]] = &[b"t", b"text"];
/// The local elements whose close ends a line, for a comments part.
///
/// The legacy format puts an entire comment's text directly in `p:text`, with
/// no paragraph wrapping it at all, so ending a line there too is what keeps
/// two consecutive comments in the same part apart; without it they run
/// together with nothing to tell where one ends and the next begins. `p`
/// still ends a line here too, for the modern format's `DrawingML`
/// paragraphs; `text` never appears in that format, so adding it here has no
/// effect on modern comments.
const COMMENT_LINE_TERMINATORS: &[&[u8]] = &[b"p", b"text"];

/// The parts that carry readable text, in reading order.
///
/// Slide layouts and masters are excluded. Their text is template prompt
/// material such as "Click to edit Master title style", which would be
/// injected into every cleaned presentation.
fn selected<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let pick = |prefix: &str| -> Vec<String> {
        let mut found: Vec<String> = names
            .iter()
            .map(AsRef::as_ref)
            .filter(|name| is_part_of(name, prefix))
            .map(str::to_owned)
            .collect();
        found.sort_by_key(|name| (index_of(name), name.clone()));
        found
    };

    let mut parts = pick(SLIDES);
    parts.extend(pick(NOTES));
    parts.extend(pick(COMMENTS));
    parts
}

/// Whether `name` is an XML part directly inside `prefix`.
fn is_part_of(name: &str, prefix: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix) else {
        return false;
    };
    !rest.contains('/') && rest.to_ascii_lowercase().ends_with(".xml")
}

/// The trailing number in a part name, for numeric ordering.
///
/// A part with no number sorts first, and ties fall back to the name, so the
/// order is total rather than dependent on the archive's listing.
fn index_of(name: &str) -> u32 {
    let digits: String = name
        .trim_end_matches(|character: char| !character.is_ascii_digit())
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// Reads every slide, speaker note and comment in reading order.
///
/// # Errors
///
/// Returns an error if the file is not a readable archive, if it holds no
/// slide part, if a part cannot be read or parsed, or if it yields no text.
pub fn to_text(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable .pptx archive", path.display()))?;

    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let parts = selected(&names);
    if !parts.iter().any(|part| part.starts_with(SLIDES)) {
        bail!(
            "{} has no {SLIDES} part; it may be an older .ppt renamed to .pptx",
            path.display()
        );
    }

    let mut text = String::new();
    for part in parts {
        let xml = super::xml::read_part(&mut archive, &part, path)?;
        let (elements, terminators): (&[&[u8]], &[&[u8]]) = if part.starts_with(COMMENTS) {
            (COMMENT_TEXT, COMMENT_LINE_TERMINATORS)
        } else {
            (&[b"t"], &[b"p"])
        };
        let extracted = super::xml::runs(&xml, elements, terminators)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A deck's parts are numbered, and a real 31-slide deck sorts lexically
    /// as slide1, slide10, slide11, which would put most of the deck before
    /// its second slide.
    #[test]
    fn slides_are_ordered_numerically_not_lexically() {
        let names = [
            "ppt/slides/slide10.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/slide1.xml",
        ];
        assert_eq!(
            selected(&names),
            vec![
                "ppt/slides/slide1.xml",
                "ppt/slides/slide2.xml",
                "ppt/slides/slide10.xml"
            ]
        );
    }

    #[test]
    fn layouts_and_masters_are_not_read() {
        let names = [
            "ppt/slides/slide1.xml",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/theme/theme1.xml",
        ];
        assert_eq!(selected(&names), vec!["ppt/slides/slide1.xml"]);
    }

    #[test]
    fn notes_follow_the_slides() {
        let names = ["ppt/notesSlides/notesSlide1.xml", "ppt/slides/slide1.xml"];
        assert_eq!(
            selected(&names),
            vec!["ppt/slides/slide1.xml", "ppt/notesSlides/notesSlide1.xml"]
        );
    }

    #[test]
    fn a_file_that_is_not_a_zip_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.pptx");
        std::fs::write(&path, "this is not a zip").expect("writing");
        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable .pptx"));
    }

    /// A deck holding notes or comments but no slide part must still be
    /// refused, and the message it gives must name the part that is missing
    /// rather than claim the whole archive is empty.
    #[test]
    fn an_archive_with_notes_but_no_slides_is_refused() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("notes-only.pptx");
        let file = std::fs::File::create(&path).expect("creating");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("ppt/notesSlides/notesSlide1.xml", options)
            .expect("starting entry");
        writer
            .write_all(
                br#"<?xml version="1.0"?><p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:t>note</a:t></p:notes>"#,
            )
            .expect("writing entry");
        writer.finish().expect("finishing archive");

        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains(SLIDES));
    }

    /// A real commented deck names the part `modernComment_100_0.xml`, which a
    /// `comment*.xml` glob does not match, so matching on the prefix would
    /// read no comments at all while appearing to work.
    #[test]
    fn a_modern_comment_part_is_selected_despite_its_name() {
        let names = [
            "ppt/slides/slide1.xml",
            "ppt/comments/modernComment_100_0.xml",
        ];
        assert_eq!(
            selected(&names),
            vec![
                "ppt/slides/slide1.xml",
                "ppt/comments/modernComment_100_0.xml"
            ]
        );
    }

    /// The modern format wraps ordinary `DrawingML` in `p188:txBody`, so the
    /// shared run reader handles it with no special rule.
    #[test]
    fn modern_comment_text_is_extracted() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p188:cmLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p188="http://schemas.microsoft.com/office/powerpoint/2018/8/main"><p188:cm id="{325EE183}" authorId="{FCDEFDF0}" created="2026-07-30T08:15:29.717"><p188:pos x="9028020" y="381907"/><p188:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-FR"/><a:t>jean.dupont@acme-consulting.example</a:t></a:r></a:p></p188:txBody></p188:cm></p188:cmLst>"#;
        let text =
            super::super::xml::runs(xml, COMMENT_TEXT, COMMENT_LINE_TERMINATORS).expect("parsing");
        assert!(
            text.contains("jean.dupont@acme-consulting.example"),
            "comment text dropped:\n{text}"
        );
    }

    /// The legacy format puts comment text directly in `p:text` rather than in
    /// a run, so a reader keyed only on `t` would read the part as empty.
    #[test]
    fn legacy_comment_text_is_extracted() {
        let xml = r#"<?xml version="1.0"?>
<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cm authorId="1" idx="1"><p:pos x="1" y="1"/><p:text>06 12 34 56 78</p:text></p:cm></p:cmLst>"#;
        let text =
            super::super::xml::runs(xml, COMMENT_TEXT, COMMENT_LINE_TERMINATORS).expect("parsing");
        assert!(
            text.contains("06 12 34 56 78"),
            "legacy comment text dropped:\n{text}"
        );
    }

    /// `p:text` holds a whole legacy comment with no paragraph wrapping it, so
    /// a `ppt/comments/comment1.xml` holding two comments must not run them
    /// together: a phone number ending one comment and the start of the next
    /// merging is exactly how a value reaches output undetected.
    #[test]
    fn two_consecutive_legacy_comments_do_not_merge() {
        let xml = r#"<?xml version="1.0"?>
<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cm authorId="1" idx="1"><p:pos x="1" y="1"/><p:text>06 12 34 56 78</p:text></p:cm><p:cm authorId="1" idx="2"><p:pos x="1" y="1"/><p:text>12 bis rue de la Paix</p:text></p:cm></p:cmLst>"#;
        let text =
            super::super::xml::runs(xml, COMMENT_TEXT, COMMENT_LINE_TERMINATORS).expect("parsing");
        assert!(
            text.contains("06 12 34 56 78"),
            "the first comment was lost:\n{text}"
        );
        assert!(
            text.contains("12 bis rue de la Paix"),
            "the second comment was lost:\n{text}"
        );
        assert!(
            !text.contains("06 12 34 56 7812 bis"),
            "the two comments merged into one candidate:\n{text}"
        );
    }

    /// `ppt/authors.xml` holds the commenter's name, but in attributes rather
    /// than character data, so it is never extracted and cannot reach output.
    #[test]
    fn the_author_list_is_not_read() {
        let names = ["ppt/slides/slide1.xml", "ppt/authors.xml"];
        assert_eq!(selected(&names), vec!["ppt/slides/slide1.xml"]);
    }
}
