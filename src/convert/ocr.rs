//! Optical character recognition for images.
//!
//! Compiled only with the `ocr` feature, which needs the Tesseract and
//! Leptonica system libraries.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

/// Languages requested from Tesseract.
///
/// French first, matching the documents this tool is aimed at, with English
/// alongside so mixed correspondence still reads. Both need their trained
/// data installed.
const LANGUAGES: &str = "fra+eng";

pub fn image_to_text(path: &Path) -> Result<String> {
    if !path.is_file() {
        bail!("cannot read {}: no such file", path.display());
    }

    let mut engine = leptess::LepTess::new(None, LANGUAGES).map_err(|error| {
        anyhow!(
            "starting Tesseract with languages '{LANGUAGES}' failed: {error}. \
             Install the trained data, for example the tesseract-ocr-fra and \
             tesseract-ocr-eng packages."
        )
    })?;

    engine
        .set_image(path)
        .with_context(|| format!("loading {} as an image", path.display()))?;

    let text = engine
        .get_utf8_text()
        .with_context(|| format!("recognising text in {}", path.display()))?;

    if text.trim().is_empty() {
        bail!(
            "no text was recognised in {}. If it does contain writing, it may be too low \
             resolution to read; otherwise there is nothing here to anonymise.",
            path.display()
        );
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_image_is_reported_clearly() {
        let error = image_to_text(Path::new("/nonexistent/scan.png")).expect_err("must reject");
        assert!(format!("{error:#}").contains("no such file"));
    }

    #[test]
    fn a_file_that_is_not_an_image_is_rejected() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.png");
        std::fs::write(&path, "definitely not a png").expect("writing");
        assert!(image_to_text(&path).is_err());
    }
}
