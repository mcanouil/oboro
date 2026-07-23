//! Conversion of input files into the plain text the detectors work on.
//!
//! The guiding rule is that a conversion either produces the document's real
//! text or fails. Returning a fraction of a document would hand the detectors
//! less than the user is about to share, and produce output that looks
//! sanitised without ever having been read.

mod docx;
mod pdf;
mod xlsx;

#[cfg(feature = "ocr")]
mod ocr;

use std::path::Path;

use anyhow::{Context, Result, bail};

/// A format this build knows how to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Plain text and markdown, tidied by [`tidy`] as it is read.
    Text,
    /// Comma-separated values, read as-is; the output keeps the extension.
    Csv,
    /// Tab-separated values, read as-is; the output keeps the extension.
    Tsv,
    Docx,
    Xlsx,
    Pdf,
    Image,
}

/// The one place mapping extensions to formats.
///
/// Both [`supported`] and the dispatch in [`read`] derive from this, so a
/// format cannot be advertised without being wired up, or wired up without
/// being advertised.
const FORMATS: &[(&str, Format)] = &[
    ("txt", Format::Text),
    ("text", Format::Text),
    ("md", Format::Text),
    ("markdown", Format::Text),
    ("csv", Format::Csv),
    ("tsv", Format::Tsv),
    ("docx", Format::Docx),
    ("xlsx", Format::Xlsx),
    ("xlsm", Format::Xlsx),
    ("pdf", Format::Pdf),
    ("png", Format::Image),
    ("jpg", Format::Image),
    ("jpeg", Format::Image),
    ("tif", Format::Image),
    ("tiff", Format::Image),
];

/// One sheet of a workbook, read as tab-separated rows.
#[derive(Debug)]
pub struct Sheet {
    /// Zero-based position in the workbook, kept explicitly so fallback
    /// output names (`sheet<N>`) still count skipped empty sheets.
    pub index: usize,
    /// The sheet's name as stored in the workbook. It may hold PII and
    /// path-hostile characters, so it must not be used in a path as-is; see
    /// [`crate::review::SheetNamer`].
    pub name: String,
    /// Tab-separated rows, one line per non-empty row.
    pub text: String,
}

/// What reading one input file yields.
#[derive(Debug)]
pub enum Conversion {
    /// One text document, written as `<stem>.clean.md` (or the input's own
    /// tabular extension for csv/tsv).
    Document(String),
    /// One table per sheet, each written as `<stem>.<sheet>.clean.tsv`.
    Sheets(Vec<Sheet>),
}

impl Conversion {
    /// Flattens into uniform parts, so consumers handle one shape: the
    /// sheet's zero-based position and raw name (`None` for a whole
    /// document), and the text.
    #[must_use]
    pub fn into_parts(self) -> Vec<(Option<(usize, String)>, String)> {
        match self {
            Self::Document(text) => vec![(None, text)],
            Self::Sheets(sheets) => sheets
                .into_iter()
                .map(|sheet| (Some((sheet.index, sheet.name)), sheet.text))
                .collect(),
        }
    }
}

/// Every suffix a cleaned output can carry, for excluding outputs on walks.
pub const OUTPUT_SUFFIXES: &[&str] = &[".clean.md", ".clean.tsv", ".clean.csv"];

/// The output suffix for a given input format.
///
/// Tabular inputs keep a tabular extension so the output opens in a
/// spreadsheet tool; everything else, including unrecognised formats, becomes
/// markdown.
#[must_use]
pub fn output_suffix(format: Option<Format>) -> &'static str {
    match format {
        Some(Format::Csv) => ".clean.csv",
        Some(Format::Tsv | Format::Xlsx) => ".clean.tsv",
        _ => ".clean.md",
    }
}

/// Whether this build can perform optical character recognition.
#[must_use]
pub const fn ocr_available() -> bool {
    cfg!(feature = "ocr")
}

/// Extensions this build accepts, for error messages and `doctor`.
///
/// Image formats only appear when the `ocr` feature is compiled in, since
/// without it there is no way to read them.
#[must_use]
pub fn supported() -> Vec<&'static str> {
    FORMATS
        .iter()
        .filter(|(_, format)| *format != Format::Image || ocr_available())
        .map(|(extension, _)| *extension)
        .collect()
}

/// The format of `path`, by extension.
#[must_use]
pub fn format_of(path: &Path) -> Option<Format> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    FORMATS
        .iter()
        .find(|(candidate, _)| *candidate == extension)
        .map(|(_, format)| *format)
}

/// Reads `path` and returns its content, one document or one table per sheet.
///
/// Text and markdown are tidied by [`tidy`] before anything else sees them, so
/// detection spans and the written output agree. Tabular content is returned
/// byte for byte, since whitespace there is structure.
///
/// `ocr_languages` only reaches the image path, and only as a hint: left empty,
/// recognition uses whatever trained data is installed.
///
/// # Errors
///
/// Returns an error if the format is unsupported, the file cannot be read or
/// parsed, or the document holds no extractable text. That last case matters:
/// a scanned PDF silently yielding nothing would look like a document with
/// nothing sensitive in it.
pub fn read(path: &Path, ocr_languages: &[String]) -> Result<Conversion> {
    let Some(format) = format_of(path) else {
        bail!(
            "unsupported file type for {}; this build reads: {}",
            path.display(),
            supported().join(", ")
        );
    };

    match format {
        Format::Text => read_utf8(path).map(|text| Conversion::Document(tidy(&text))),
        Format::Csv | Format::Tsv => read_utf8(path).map(Conversion::Document),
        Format::Docx => docx::to_text(path).map(Conversion::Document),
        Format::Xlsx => xlsx::to_sheets(path).map(Conversion::Sheets),
        Format::Pdf => pdf::to_text(path).map(Conversion::Document),
        Format::Image => image_to_text(path, ocr_languages).map(Conversion::Document),
    }
}

fn read_utf8(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("reading {}; it must be valid UTF-8 text", path.display()))
}

/// Strips trailing whitespace from every line, collapses runs of empty lines
/// into one, and drops empty lines at either end, ending on a single newline.
///
/// Leading whitespace is left alone: in markdown it carries structure, such as
/// nested list items and indented code blocks. Nothing here parses markdown,
/// so a hard line break written as two trailing spaces does not survive, and
/// blank lines inside a fenced code block collapse like any other.
fn tidy(text: &str) -> String {
    let mut tidied = String::with_capacity(text.len());
    let mut blank_pending = false;
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() {
            blank_pending = !tidied.is_empty();
            continue;
        }
        if blank_pending {
            tidied.push('\n');
            blank_pending = false;
        }
        tidied.push_str(line);
        tidied.push('\n');
    }
    tidied
}

#[cfg(feature = "ocr")]
fn image_to_text(path: &Path, languages: &[String]) -> Result<String> {
    ocr::image_to_text(path, languages)
}

#[cfg(not(feature = "ocr"))]
fn image_to_text(path: &Path, _languages: &[String]) -> Result<String> {
    bail!(
        "cannot read {}: reading images needs optical character recognition, \
         which this build was compiled without. Rebuild with `--features ocr` \
         after installing Tesseract.",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads `path`, asserting it yields a single document.
    fn read_document(path: &Path) -> Result<String> {
        match read(path, &[])? {
            Conversion::Document(text) => Ok(text),
            Conversion::Sheets(_) => panic!("{} unexpectedly read as sheets", path.display()),
        }
    }

    #[test]
    fn reads_supported_text_files() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n").expect("writing");
        assert_eq!(read_document(&path).expect("reading"), "# Title\n");
    }

    #[test]
    fn reads_csv_and_tsv_as_plain_text() {
        let dir = tempfile::tempdir().expect("temporary directory");
        for name in ["data.csv", "data.tsv"] {
            let path = dir.path().join(name);
            std::fs::write(&path, "name,mail\nJean,a@example.com\n").expect("writing");
            assert_eq!(
                read_document(&path).expect("reading"),
                "name,mail\nJean,a@example.com\n"
            );
        }
    }

    #[test]
    fn tabular_content_is_never_tidied() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let raw = "name,mail  \n\n\n,\nJean,a@example.com\n\n";
        for name in ["data.csv", "data.tsv"] {
            let path = dir.path().join(name);
            std::fs::write(&path, raw).expect("writing");
            assert_eq!(
                read_document(&path).expect("reading"),
                raw,
                "{name}: trimming a cell or dropping a row would move the columns"
            );
        }
    }

    /// Reads `content` written to a markdown file, as the tidying pass leaves it.
    fn read_tidied(content: &str) -> String {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("note.md");
        std::fs::write(&path, content).expect("writing");
        read_document(&path).expect("reading")
    }

    #[test]
    fn trailing_whitespace_goes_and_indentation_stays() {
        assert_eq!(
            read_tidied("- one  \n  - two\t\n\n      code  \n"),
            "- one\n  - two\n\n      code\n"
        );
    }

    #[test]
    fn runs_of_empty_lines_collapse_into_one() {
        assert_eq!(read_tidied("# Title\n\n\n\nBody\n"), "# Title\n\nBody\n");
    }

    #[test]
    fn blank_lines_at_either_end_are_dropped() {
        assert_eq!(read_tidied("\n  \n# Title\n\n \n\n"), "# Title\n");
    }

    #[test]
    fn text_ends_on_exactly_one_newline() {
        assert_eq!(read_tidied("# Title"), "# Title\n");
        assert_eq!(read_tidied("# Title\n\n\n"), "# Title\n");
    }

    #[test]
    fn a_file_of_only_whitespace_reads_as_empty_text() {
        assert_eq!(read_tidied("  \n\t\n\n"), "");
    }

    #[test]
    fn carriage_returns_do_not_survive() {
        assert_eq!(read_tidied("# Title\r\n\r\nBody\r\n"), "# Title\n\nBody\n");
    }

    #[test]
    fn tidying_an_already_tidy_document_changes_nothing() {
        let tidy_text = "# Title\n\n- one\n  - two\n\nBody\n";
        assert_eq!(read_tidied(tidy_text), tidy_text);
    }

    #[test]
    fn reads_an_empty_file_as_empty_text() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("empty.txt");
        std::fs::write(&path, "").expect("writing");
        assert_eq!(read_document(&path).expect("reading"), "");
    }

    #[test]
    fn a_spreadsheet_reads_as_sheets() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join("clients.xlsx");
        match read(&path, &[]).expect("reading") {
            Conversion::Sheets(sheets) => assert!(!sheets.is_empty()),
            Conversion::Document(_) => panic!("a workbook must read as sheets"),
        }
    }

    #[test]
    fn rejects_unsupported_extensions_by_name() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("archive.zip");
        std::fs::write(&path, "binary").expect("writing");
        let error = read(&path, &[]).expect_err("zip is not supported");
        assert!(format!("{error:#}").contains("unsupported file type"));
    }

    #[test]
    fn rejects_files_without_an_extension() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("README");
        std::fs::write(&path, "text").expect("writing");
        assert!(read(&path, &[]).is_err());
    }

    #[test]
    fn reports_a_missing_file_with_its_path() {
        let error = read(Path::new("/nonexistent/file.txt"), &[]).expect_err("missing file");
        assert!(format!("{error:#}").contains("file.txt"));
    }

    #[test]
    fn rejects_non_utf8_text_content() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("latin.txt");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("writing");
        assert!(read(&path, &[]).is_err());
    }

    #[test]
    fn extensions_are_matched_regardless_of_case() {
        assert_eq!(format_of(Path::new("a.PDF")), Some(Format::Pdf));
        assert_eq!(format_of(Path::new("a.DocX")), Some(Format::Docx));
        assert_eq!(format_of(Path::new("a.CSV")), Some(Format::Csv));
        assert_eq!(format_of(Path::new("a.tsv")), Some(Format::Tsv));
        assert_eq!(format_of(Path::new("a.zip")), None);
    }

    #[test]
    fn tabular_extensions_are_advertised() {
        assert!(supported().contains(&"csv"));
        assert!(supported().contains(&"tsv"));
    }

    #[test]
    fn every_output_suffix_is_listed_for_walk_exclusion() {
        let formats = [
            None,
            Some(Format::Text),
            Some(Format::Csv),
            Some(Format::Tsv),
            Some(Format::Docx),
            Some(Format::Xlsx),
            Some(Format::Pdf),
            Some(Format::Image),
        ];
        for format in formats {
            let suffix = output_suffix(format);
            assert!(
                OUTPUT_SUFFIXES.contains(&suffix),
                "{suffix} missing from OUTPUT_SUFFIXES; walks would re-clean outputs"
            );
        }
    }

    #[test]
    fn image_support_is_advertised_only_when_it_works() {
        assert_eq!(supported().contains(&"png"), ocr_available());
    }

    #[cfg(not(feature = "ocr"))]
    #[test]
    fn images_explain_how_to_enable_reading_them() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("scan.png");
        std::fs::write(&path, [0x89, b'P', b'N', b'G']).expect("writing");
        let error = read(&path, &[]).expect_err("no ocr in this build");
        assert!(format!("{error:#}").contains("--features ocr"));
    }

    proptest::proptest! {
        /// Tidying may only ever drop whitespace. Were it to alter, reorder or
        /// escape a character, a value would no longer match the rule meant to
        /// find it, and would reach the model unredacted.
        #[test]
        fn tidying_leaves_every_other_character_untouched(text in "[ \t\r\n\\PC]{0,256}") {
            let bare = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            proptest::prop_assert_eq!(bare(&tidy(&text)), bare(&text));
        }

        /// Tidying may only ever remove lines. A formatter that reflowed prose
        /// could put a line break inside an IBAN, a card number or an address,
        /// none of which are matched across one; see `detect::rules`.
        #[test]
        fn tidying_never_splits_a_line_in_two(text in "[ \t\r\n\\PC]{0,256}") {
            proptest::prop_assert!(tidy(&text).lines().count() <= text.lines().count());
        }

        /// Cleaning an already cleaned document must be a no-op, so output fed
        /// back in is left alone.
        #[test]
        fn tidying_twice_changes_nothing_more(text in "[ \t\r\n\\PC]{0,256}") {
            let once = tidy(&text);
            proptest::prop_assert_eq!(tidy(&once), once.clone());
        }
    }
}
