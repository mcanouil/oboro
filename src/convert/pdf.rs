//! PDF text extraction.
//!
//! Text-based PDFs are read directly. A page carrying no text is a scan, and
//! on a build with the `ocr` feature its images are recognised instead. Where
//! that is not possible the file is refused rather than passed on as a handful
//! of stray characters: a document that looks sanitised but was never actually
//! read is the worst outcome this tool has.
//!
//! Recognition works on the images already embedded in the page rather than by
//! rasterising it, which is what a scan is: one image per page, put there by
//! the scanner. Pages drawn some other way are not reached, and are refused as
//! before.

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

pub fn to_text(path: &Path, ocr_languages: &[String]) -> Result<String> {
    let document = load(path)?;
    let pages = document.get_pages().len();
    let text = extract(path)?;
    let visible = text.chars().filter(|c| !c.is_whitespace()).count();

    if visible >= MIN_CHARS_PER_PAGE.saturating_mul(pages.max(1)) {
        return Ok(text);
    }

    recognise(&document, path, ocr_languages, visible, pages)
}

/// Reads the page images and returns whatever they say.
///
/// Reached only once direct extraction has come up short, so an empty result
/// means the document cannot be read at all, not that it holds nothing worth
/// redacting.
#[cfg(feature = "ocr")]
fn recognise(
    document: &lopdf::Document,
    path: &Path,
    languages: &[String],
    visible: usize,
    pages: usize,
) -> Result<String> {
    let images = page_images(document, path)?;
    let text = super::ocr::images_to_text(&images, languages)?;

    if text.trim().is_empty() {
        bail!(
            "{} yielded only {visible} characters across {pages} page(s), and reading the \
             images on its pages recognised no text either. Returning it would produce \
             output that looks sanitised without having been read.",
            path.display()
        );
    }
    Ok(text)
}

#[cfg(not(feature = "ocr"))]
fn recognise(
    _document: &lopdf::Document,
    path: &Path,
    _languages: &[String],
    visible: usize,
    pages: usize,
) -> Result<String> {
    bail!(
        "{} yielded only {visible} characters across {pages} page(s), so it is almost \
         certainly scanned images rather than text. Reading it would produce output that \
         looks sanitised without having been read. Reading the pages needs optical \
         character recognition, which this build was compiled without: rebuild with \
         `--features ocr` after installing Tesseract.",
        path.display()
    )
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

fn load(path: &Path) -> Result<lopdf::Document> {
    lopdf::Document::load(path).with_context(|| format!("{} is not a readable PDF", path.display()))
}

/// Image filters Leptonica reads from the bytes the PDF already holds, so the
/// stream goes straight through without being decoded here.
#[cfg(feature = "ocr")]
const PASSTHROUGH_FILTERS: [&str; 2] = ["DCTDecode", "JPXDecode"];

/// Below this in either dimension, an image is page furniture rather than a
/// scan.
///
/// Writers emit one-pixel soft masks and spacers beside the real page image,
/// and refusing a document over one of those would make readable scans
/// unreadable for no reason.
#[cfg(feature = "ocr")]
const MIN_SCAN_PIXELS: i64 = 16;

/// Every page image in the document, in page order, encoded as something
/// Leptonica can open.
///
/// An image in a codec that cannot be handed over is an error rather than a
/// skip: reading the rest of a document and quietly leaving one page out is
/// the half-read outcome this module exists to prevent.
#[cfg(feature = "ocr")]
fn page_images(document: &lopdf::Document, path: &Path) -> Result<Vec<Vec<u8>>> {
    let mut images = Vec::new();
    for (number, id) in document.get_pages() {
        let found = document.get_page_images(id).with_context(|| {
            format!("reading the images on page {number} of {}", path.display())
        })?;
        for image in found {
            if image.width < MIN_SCAN_PIXELS || image.height < MIN_SCAN_PIXELS {
                continue;
            }
            images.push(readable(&image, number, path)?);
        }
    }
    Ok(images)
}

/// Turns one page image into bytes Leptonica opens.
#[cfg(feature = "ocr")]
fn readable(image: &lopdf::xobject::PdfImage, page: u32, path: &Path) -> Result<Vec<u8>> {
    let filters = image.filters.as_deref().unwrap_or_default();

    if filters
        .iter()
        .any(|filter| PASSTHROUGH_FILTERS.contains(&filter.as_str()))
    {
        return Ok(image.content.to_vec());
    }

    if filters.iter().any(|filter| filter == "CCITTFaxDecode") {
        return fax_as_tiff(image, page, path);
    }

    bail!(
        "page {page} of {} holds an image encoded with {}, which cannot be read. \
         Only DCTDecode, JPXDecode and CCITTFaxDecode images are recognised.",
        path.display(),
        if filters.is_empty() {
            "no filter".to_owned()
        } else {
            filters.join(", ")
        }
    )
}

/// Wraps an undecoded CCITT fax stream in a TIFF header.
///
/// TIFF carries Group 3 and Group 4 natively, so this rewraps rather than
/// decodes: the fax bytes are copied across untouched and only the header
/// describing them is built here.
#[cfg(feature = "ocr")]
fn fax_as_tiff(image: &lopdf::xobject::PdfImage, page: u32, path: &Path) -> Result<Vec<u8>> {
    /// TIFF field types, of which only these two are needed.
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    /// Where the fax data sits, immediately after the eight-byte header.
    const DATA_OFFSET: u32 = 8;

    let parameters = fax_parameters(image);
    let describe =
        |what: &str| format!("page {page} of {} has an impossible {what}", path.display());

    // The stream is coded against Columns, so that is the width to describe it
    // by. Falling back to the image's own width rather than the 1728 the
    // specification defaults to: a writer omitting Columns for a page that is
    // not fax-width meant the page's width.
    let columns = u32::try_from(parameters.columns.unwrap_or(image.width))
        .with_context(|| describe("width"))?;
    let rows = u32::try_from(parameters.rows.unwrap_or(image.height))
        .with_context(|| describe("height"))?;
    let bytes = u32::try_from(image.content.len()).with_context(|| describe("image size"))?;

    // Group 4 is a compression of its own; both flavours of Group 3 share one,
    // and are told apart by a bit in T4Options.
    let group_4 = parameters.k < 0;
    let compression = if group_4 { 4 } else { 3 };

    let mut fields = vec![
        field(256, LONG, columns),
        field(257, LONG, rows),
        field(258, SHORT, 1),
        field(259, SHORT, compression),
        // BlackIs1 says a zero bit is black, which TIFF spells BlackIsZero.
        field(262, SHORT, u32::from(!parameters.black_is_1)),
        field(273, LONG, DATA_OFFSET),
        field(278, LONG, rows),
        field(279, LONG, bytes),
    ];
    if !group_4 {
        fields.push(field(292, LONG, u32::from(parameters.k > 0)));
    }

    let count = u16::try_from(fields.len()).expect("the field count is a fixed handful");
    let mut tiff = Vec::with_capacity(image.content.len() + fields.len() * 12 + 32);
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42u16.to_le_bytes());
    tiff.extend_from_slice(&(DATA_OFFSET + bytes).to_le_bytes());
    tiff.extend_from_slice(image.content);
    tiff.extend_from_slice(&count.to_le_bytes());
    for entry in fields {
        tiff.extend_from_slice(&entry);
    }
    // No second directory follows.
    tiff.extend_from_slice(&0u32.to_le_bytes());
    Ok(tiff)
}

/// One TIFF directory entry holding a single value.
///
/// Values of both types used here fit the entry's four-byte value field, and
/// in a little-endian file a short written as four bytes lands in the right
/// half of it.
#[cfg(feature = "ocr")]
fn field(tag: u16, kind: u16, value: u32) -> [u8; 12] {
    let mut entry = [0u8; 12];
    entry[0..2].copy_from_slice(&tag.to_le_bytes());
    entry[2..4].copy_from_slice(&kind.to_le_bytes());
    entry[4..8].copy_from_slice(&1u32.to_le_bytes());
    entry[8..12].copy_from_slice(&value.to_le_bytes());
    entry
}

/// What `/DecodeParms` says about a fax stream.
#[cfg(feature = "ocr")]
struct FaxParameters {
    /// Which coding was used: negative for Group 4, zero for one-dimensional
    /// Group 3, positive for mixed Group 3.
    k: i64,
    /// Whether a one bit means black. The default is that a zero does.
    black_is_1: bool,
    columns: Option<i64>,
    rows: Option<i64>,
}

/// Reads `/DecodeParms`, falling back to the defaults the specification gives.
///
/// Every entry is optional and a missing one is not an error, so this reports
/// no failure: a stream whose parameters are all absent still decodes on the
/// defaults.
#[cfg(feature = "ocr")]
fn fax_parameters(image: &lopdf::xobject::PdfImage) -> FaxParameters {
    // Written either as one dictionary or as an array matching `/Filter`, in
    // which case the fax parameters are the only dictionary in it.
    let parameters = image
        .origin_dict
        .get(b"DecodeParms")
        .or_else(|_| image.origin_dict.get(b"DP"))
        .ok()
        .and_then(|object| {
            object.as_dict().ok().or_else(|| {
                object
                    .as_array()
                    .ok()?
                    .iter()
                    .find_map(|entry| entry.as_dict().ok())
            })
        });

    let Some(parameters) = parameters else {
        return FaxParameters {
            k: 0,
            black_is_1: false,
            columns: None,
            rows: None,
        };
    };

    FaxParameters {
        k: parameters
            .get(b"K")
            .and_then(lopdf::Object::as_i64)
            .unwrap_or(0),
        black_is_1: parameters
            .get(b"BlackIs1")
            .and_then(lopdf::Object::as_bool)
            .unwrap_or(false),
        columns: parameters
            .get(b"Columns")
            .and_then(lopdf::Object::as_i64)
            .ok(),
        rows: parameters.get(b"Rows").and_then(lopdf::Object::as_i64).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    /// A one-page PDF whose page is a single image in a codec nothing here
    /// decodes.
    ///
    /// Built rather than committed: no one needs to look at it, and the point
    /// is the entry in `/Filter`, not the bytes it claims to describe.
    #[cfg(feature = "ocr")]
    fn undecodable_page() -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let image = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 640,
                "Height" => 480,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 1,
                "Filter" => "JBIG2Decode",
            },
            b"not actually jbig2".to_vec(),
        ));
        let contents = document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let pages = document.new_object_id();
        let page = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => contents,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
        });
        document.objects.insert(
            pages,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages,
        });
        document.trailer.set("Root", catalog);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("writing the pdf");
        bytes
    }

    /// A one-line invoice is short but perfectly readable, and refusing it
    /// would block a legitimate document.
    #[test]
    fn a_sparse_but_genuine_document_is_read() {
        let text =
            to_text(&fixture("sparse.pdf"), &[]).expect("a short document must still be read");
        assert!(text.contains("FR14"), "expected the IBAN, got:\n{text}");
    }

    /// A page yielding nothing must be refused, since output that looks
    /// sanitised but was never read is the worst outcome here. Recognition
    /// does not change this: there is nothing on the page to recognise.
    #[test]
    fn a_page_with_no_text_is_refused() {
        let error = to_text(&fixture("scanned.pdf"), &[]).expect_err("must refuse");
        assert!(format!("{error:#}").contains("scanned"));
    }

    #[test]
    fn a_file_that_is_not_a_pdf_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.pdf");
        std::fs::write(&path, "not a pdf at all").expect("writing");
        let error = to_text(&path, &[]).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable PDF"));
    }

    #[test]
    fn a_missing_pdf_names_the_file() {
        let error = to_text(Path::new("/nonexistent/report.pdf"), &[]).expect_err("must reject");
        assert!(format!("{error:#}").contains("report.pdf"));
    }

    /// Without recognition there is nothing to fall back on, so the refusal
    /// has to say how to get it rather than leaving the user guessing.
    #[cfg(not(feature = "ocr"))]
    #[test]
    fn a_scanned_page_says_how_to_enable_reading_it() {
        let error = to_text(&fixture("scan.pdf"), &[]).expect_err("must refuse without ocr");
        assert!(format!("{error:#}").contains("--features ocr"));
    }

    /// The common scan: one full-page JPEG per page, handed to Tesseract
    /// untouched because Leptonica reads the codec itself.
    #[cfg(feature = "ocr")]
    #[test]
    fn a_scanned_page_stored_as_a_jpeg_is_recognised() {
        let text = to_text(&fixture("scan.pdf"), &[]).expect("a scanned page must be read");
        assert!(
            text.contains("FR14 2004 1010 0505 0001 3M02 606"),
            "expected the IBAN, got:\n{text}"
        );
        assert!(
            text.contains("Acme Consulting SARL"),
            "expected the provider name, got:\n{text}"
        );
    }

    /// The other common scan: a bilevel page kept as Group 4 fax data, which
    /// reaches Tesseract only through the TIFF wrapper. A wrapper that
    /// inverted the image would recognise nothing, so this is what pins the
    /// polarity down.
    #[cfg(feature = "ocr")]
    #[test]
    fn a_scanned_page_stored_as_group_4_fax_data_is_recognised() {
        let text = to_text(&fixture("scan-fax.pdf"), &[]).expect("a fax-coded page must be read");
        assert!(
            text.contains("FR14 2004 1010 0505 0001 3M02 606"),
            "expected the IBAN, got:\n{text}"
        );
        assert!(
            text.contains("Acme Consulting SARL"),
            "expected the provider name, got:\n{text}"
        );
    }

    /// A page carrying an image nothing here can decode must be refused by
    /// name, not read as the nothing that surrounds the image.
    #[cfg(feature = "ocr")]
    #[test]
    fn an_image_in_an_unreadable_codec_is_refused_by_name() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("jbig2.pdf");
        std::fs::write(&path, undecodable_page()).expect("writing");
        let error = to_text(&path, &[]).expect_err("must refuse an undecodable codec");
        assert!(
            format!("{error:#}").contains("JBIG2Decode"),
            "the error must name the codec: {error:#}"
        );
    }
}
