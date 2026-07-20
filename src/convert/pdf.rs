//! PDF text extraction.
//!
//! Text-based PDFs are read directly. Scanned ones are refused rather than
//! passed on as a handful of stray characters: a document that looks
//! sanitised but was never actually read is the worst outcome this tool has.

use std::panic::AssertUnwindSafe;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

/// Below this many characters per page, a PDF is treated as scanned rather
/// than as a document whose text could not be read.
///
/// Deliberately low. The case worth catching is a page that yields nothing at
/// all, and a legitimately sparse document is a real thing: a single-line
/// invoice carrying just an IBAN clears thirty characters and must be read,
/// not refused. Set high enough to catch stray page furniture, no higher.
const MIN_CHARS_PER_PAGE: usize = 8;

pub fn to_text(path: &Path) -> Result<String> {
    let pages = page_count(path)?;
    let text = extract(path)?;
    let visible = text.chars().filter(|c| !c.is_whitespace()).count();

    if visible < MIN_CHARS_PER_PAGE.saturating_mul(pages.max(1)) {
        bail!(
            "{} yielded only {visible} characters across {pages} page(s), so it is almost \
             certainly scanned images rather than text. Reading it would produce output that \
             looks sanitised without having been read. Export the pages as images and pass \
             those instead{}.",
            path.display(),
            if super::ocr_available() {
                ""
            } else {
                ", using a build with `--features ocr`"
            }
        );
    }

    Ok(text)
}

/// Runs the extractor, containing any panic it might have on malformed input.
///
/// The parser is third-party code being fed documents from wherever the user
/// got them, so a crash is a plausible outcome and a poor one: it would give
/// no indication whether the file was read.
fn extract(path: &Path) -> Result<String> {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text(path)));
    match outcome {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(error)) => {
            Err(anyhow!(error)).with_context(|| format!("reading text from {}", path.display()))
        }
        Err(_) => bail!(
            "the PDF parser crashed on {}; the file is malformed or uses an unsupported feature",
            path.display()
        ),
    }
}

fn page_count(path: &Path) -> Result<usize> {
    let document = lopdf::Document::load(path)
        .with_context(|| format!("{} is not a readable PDF", path.display()))?;
    Ok(document.get_pages().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    /// A one-line invoice is short but perfectly readable, and refusing it
    /// would block a legitimate document.
    #[test]
    fn a_sparse_but_genuine_document_is_read() {
        let text = to_text(&fixture("sparse.pdf")).expect("a short document must still be read");
        assert!(text.contains("FR14"), "expected the IBAN, got:\n{text}");
    }

    /// A page yielding nothing must be refused, since output that looks
    /// sanitised but was never read is the worst outcome here.
    #[test]
    fn a_page_with_no_text_is_refused() {
        let error = to_text(&fixture("scanned.pdf")).expect_err("must refuse");
        assert!(format!("{error:#}").contains("scanned"));
    }

    #[test]
    fn a_file_that_is_not_a_pdf_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.pdf");
        std::fs::write(&path, "not a pdf at all").expect("writing");
        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable PDF"));
    }

    #[test]
    fn a_missing_pdf_names_the_file() {
        let error = to_text(Path::new("/nonexistent/report.pdf")).expect_err("must reject");
        assert!(format!("{error:#}").contains("report.pdf"));
    }
}
