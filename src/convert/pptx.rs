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
//!
//! Nothing calls [`to_text`] yet: the dispatch that reaches it belongs to a
//! later step of the pptx/odt reader work, which is why the module is
//! otherwise complete but unreachable. Each item below carries its own
//! `#[allow(dead_code)]` for that reason; remove all six once a later step
//! adds the `Format::Pptx` dispatch arm that calls [`to_text`].

use std::path::Path;

use anyhow::{Context, Result, bail};

/// Where the slides live. Also the marker for a readable presentation.
#[allow(dead_code)]
const SLIDES: &str = "ppt/slides/";
/// Speaker notes, read because they are text the author wrote.
#[allow(dead_code)]
const NOTES: &str = "ppt/notesSlides/";

/// The parts that carry readable text, in reading order.
///
/// Slide layouts and masters are excluded. Their text is template prompt
/// material such as "Click to edit Master title style", which would be
/// injected into every cleaned presentation.
#[allow(dead_code)]
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
    parts
}

/// Whether `name` is an XML part directly inside `prefix`.
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Reads every slide and note in reading order.
///
/// # Errors
///
/// Returns an error if the file is not a readable archive, if it holds no
/// slide part, if a part cannot be read or parsed, or if it yields no text.
#[allow(dead_code)]
pub fn to_text(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("{} is not a readable .pptx archive", path.display()))?;

    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let parts = selected(&names);
    if parts.is_empty() {
        bail!(
            "{} has no {SLIDES} part; it may be an older .ppt renamed to .pptx",
            path.display()
        );
    }

    let mut text = String::new();
    for part in parts {
        let xml = super::xml::read_part(&mut archive, &part, path)?;
        let extracted = super::xml::runs(&xml, &[b"t"])
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
}
