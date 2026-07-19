//! Conversion of input files into the plain text the detectors work on.
//!
//! Later phases add office documents, PDFs and images; this phase handles the
//! formats that need no decoding.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// Extensions understood by this build, for error messages and `doctor`.
pub const SUPPORTED: &[&str] = &["txt", "md", "markdown", "text"];

/// Reads `path` and returns its text content.
///
/// Unsupported formats fail rather than being read as bytes, because
/// silently mis-decoding a document would hand the detectors gibberish and
/// produce output that looks sanitised but is not.
///
/// # Errors
///
/// Returns an error if the extension is not supported, the file cannot be
/// read, or its contents are not valid UTF-8.
pub fn to_text(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if !SUPPORTED.contains(&extension.as_str()) {
        bail!(
            "unsupported file type '{}' for {}; this build reads: {}",
            if extension.is_empty() {
                "(none)"
            } else {
                &extension
            },
            path.display(),
            SUPPORTED.join(", ")
        );
    }

    std::fs::read_to_string(path)
        .with_context(|| format!("reading {}; it must be valid UTF-8 text", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_supported_text_files() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n").expect("writing");
        assert_eq!(to_text(&path).expect("reading"), "# Title\n");
    }

    #[test]
    fn reads_an_empty_file_as_empty_text() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, "").expect("writing");
        assert_eq!(to_text(&path).expect("reading"), "");
    }

    #[test]
    fn rejects_unsupported_extensions_by_name() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("contract.docx");
        std::fs::write(&path, "binary").expect("writing");
        let error = to_text(&path).expect_err("docx is not supported yet");
        assert!(format!("{error:#}").contains("docx"));
    }

    #[test]
    fn rejects_files_without_an_extension() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("README");
        std::fs::write(&path, "text").expect("writing");
        assert!(to_text(&path).is_err());
    }

    #[test]
    fn reports_a_missing_file_with_its_path() {
        let error = to_text(Path::new("/nonexistent/file.txt")).expect_err("missing file");
        assert!(format!("{error:#}").contains("file.txt"));
    }

    #[test]
    fn rejects_non_utf8_content() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("latin.txt");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("writing");
        assert!(to_text(&path).is_err());
    }
}
