//! Reading the images a scanned PDF page is made of.
//!
//! A scan is one image per page, put there by the scanner, so the text is
//! recovered from the images the page already embeds rather than by
//! rasterising it. This turns a page into bytes the recogniser can open, and
//! refuses by name what it cannot.
//!
//! Kept apart from the reader in [`super`] because little of it is about PDF
//! text: the resource walk follows the inheritance rule the specification
//! sets out, and the fax path builds a TIFF header, which is another format
//! entirely.
//!
//! Compiled only with the `ocr` feature, which is why the whole file is gated
//! rather than each item in it.
#![cfg(feature = "ocr")]

use std::path::Path;

use anyhow::{Context, Result, bail};

/// Image filters Leptonica reads from the bytes the PDF already holds, so the
/// stream goes straight through without being decoded here.
const PASSTHROUGH_FILTERS: [&str; 2] = ["DCTDecode", "JPXDecode"];

/// Below this in either dimension, an image is page furniture rather than a
/// scan.
///
/// Writers emit one-pixel soft masks and spacers beside the real page image,
/// and refusing a document over one of those would make readable scans
/// unreadable for no reason.
const MIN_SCAN_PIXELS: i64 = 16;

/// The most pixels a fax image may claim before it is refused unread.
///
/// The dimensions are the document's to state, and the recogniser allocates
/// from them: a three-kilobyte file claiming a hundred thousand pixels square
/// costs most of a gigabyte before the library refuses it. Generous against a
/// real page, which is some 143 million pixels at 1200 dpi.
const MAX_SCAN_PIXELS: i64 = 400_000_000;

/// One image drawn on a page, as the document stores it.
///
/// Read here rather than through `lopdf::Document::get_page_images`, which
/// looks for `/Resources` on the page alone though it is inheritable, and
/// indexes an empty `/ColorSpace` array without checking. Nothing here needs
/// the colour space, so it is not read at all.
struct PageImage<'a> {
    width: i64,
    height: i64,
    filters: Vec<String>,
    content: &'a [u8],
    dictionary: &'a lopdf::Dictionary,
}

/// One page's images, encoded as something Leptonica can open.
///
/// An image in a codec that cannot be handed over is an error rather than a
/// skip: reading the rest of a document and quietly leaving one page out is
/// the half-read outcome this module exists to prevent.
pub(super) fn page_images(
    document: &lopdf::Document,
    id: lopdf::ObjectId,
    page: u32,
    path: &Path,
) -> Result<Vec<Vec<u8>>> {
    let found = image_streams(document, id);
    let scans = found
        .iter()
        .filter(|image| image.width >= MIN_SCAN_PIXELS && image.height >= MIN_SCAN_PIXELS);

    let images: Vec<Vec<u8>> = scans
        .map(|image| readable(document, image, page, path))
        .collect::<Result<_>>()?;

    if images.is_empty()
        && let Some(largest) = found
            .iter()
            // Both dimensions come from the document unchecked, so the area is
            // computed defensively: a negative pair multiplies to a positive
            // that would win, and a large pair overflows.
            .max_by_key(|image| image.width.max(0).saturating_mul(image.height.max(0)))
    {
        bail!(
            "page {page} of {} holds nothing but images too small to be a scan, the largest \
             {}x{} pixels against a floor of {MIN_SCAN_PIXELS}. Whatever the page says is \
             not in them.",
            path.display(),
            largest.width,
            largest.height
        );
    }
    Ok(images)
}

/// Every image `XObject` reachable from a page, following `/Resources` up the
/// page tree as the specification says it is inherited.
///
/// A page with no resources or no `XObject`s has no images, which is not an
/// error here: whether that leaves the document unreadable is decided by the
/// caller, which is the only place that knows what the page yielded as text.
fn image_streams(document: &lopdf::Document, id: lopdf::ObjectId) -> Vec<PageImage<'_>> {
    let mut images = Vec::new();
    let mut walk = Walk::default();
    for resource in page_resources(document, id) {
        collect_images(document, resource, 0, &mut walk, &mut images);
    }
    images
}

/// What has already been looked at while gathering one page's images.
#[derive(Default)]
struct Walk {
    /// Images already gathered, so the same one is not read twice. A page and
    /// its parent may carry the same resources, and a document can be built so
    /// that they do, turning one image into many decoded copies held at once.
    images: std::collections::HashSet<lopdf::ObjectId>,
    /// Forms already descended into, and how deep they were at the time.
    ///
    /// The depth is kept because the descent is bounded: a form reached at the
    /// limit contributes nothing, and reaching it again from higher up must
    /// try again rather than treat it as done.
    forms: std::collections::HashMap<lopdf::ObjectId, usize>,
}

impl Walk {
    /// Whether a form should be descended into from `depth`, recording it when
    /// it should.
    ///
    /// True the first time, and again when it is reached from higher up than
    /// before: the descent is bounded, so a form stopped by the limit
    /// contributed nothing and must be tried again rather than counted as
    /// read.
    fn descend(&mut self, id: Option<lopdf::ObjectId>, depth: usize) -> bool {
        let Some(id) = id else {
            return true;
        };
        if depth >= self.forms.get(&id).copied().unwrap_or(usize::MAX) {
            return false;
        }
        self.forms.insert(id, depth);
        true
    }
}

/// Adds every image reachable from one resource dictionary to `images`.
///
/// Descends into form `XObject`s, since a page may draw its scan through one
/// rather than referring to the image directly, and a page whose only image
/// sits inside a form would otherwise look like a page with no image at all.
fn collect_images<'a>(
    document: &'a lopdf::Document,
    resources: &'a lopdf::Dictionary,
    depth: usize,
    walk: &mut Walk,
    images: &mut Vec<PageImage<'a>>,
) {
    let Some(xobjects) = resources
        .get_deref(b"XObject", document)
        .ok()
        .and_then(|object| object.as_dict().ok())
    else {
        return;
    };

    for (_, value) in xobjects {
        // An XObject is named by reference in every document that is not
        // hand-written, and the reference is what identifies it across the
        // resource dictionaries it appears in.
        let id = value.as_reference().ok();
        let Some(stream) = resolved(document, value).and_then(|object| object.as_stream().ok())
        else {
            continue;
        };

        let subtype = stream
            .dict
            .get_deref(b"Subtype", document)
            .ok()
            .and_then(|object| object.as_name().ok())
            .unwrap_or_default();

        if subtype == b"Form" {
            if depth + 1 >= MAX_XOBJECT_DEPTH {
                continue;
            }
            if !walk.descend(id, depth) {
                continue;
            }
            if let Some(inner) = stream
                .dict
                .get_deref(b"Resources", document)
                .ok()
                .and_then(|object| object.as_dict().ok())
            {
                collect_images(document, inner, depth + 1, walk, images);
            }
            continue;
        }
        if subtype != b"Image" {
            continue;
        }
        if let Some(id) = id
            && !walk.images.insert(id)
        {
            continue;
        }

        let (Some(width), Some(height)) = (
            number(document, &stream.dict, b"Width"),
            number(document, &stream.dict, b"Height"),
        ) else {
            continue;
        };
        images.push(PageImage {
            width,
            height,
            filters: filters_of(document, &stream.dict),
            content: &stream.content,
            dictionary: &stream.dict,
        });
    }
}

/// How far up the page tree inherited resources are followed.
///
/// A bound rather than a judgement: real trees are shallow, and a malformed
/// `/Parent` pointing back down one would otherwise spin.
const MAX_PAGE_TREE_DEPTH: usize = 32;

/// How far form `XObject`s are followed when looking for the page's images.
///
/// Forms nest, and a document can be built so that they nest into each other
/// without end.
pub(super) const MAX_XOBJECT_DEPTH: usize = 8;

/// The resource dictionaries in force for a page, nearest first.
///
/// `/Resources` is inheritable, so a page without its own takes the nearest
/// ancestor's. Both spellings occur: the entry may be the dictionary itself or
/// a reference to it, and `lopdf::Document::get_page_resources` collects only
/// the second on an ancestor.
fn page_resources(document: &lopdf::Document, id: lopdf::ObjectId) -> Vec<&lopdf::Dictionary> {
    let mut resources = Vec::new();
    let mut node = document.get_dictionary(id).ok();
    let mut visited = std::collections::HashSet::from([id]);

    for _ in 0..MAX_PAGE_TREE_DEPTH {
        let Some(dictionary) = node else { break };
        if let Some(found) = dictionary
            .get_deref(b"Resources", document)
            .ok()
            .and_then(|object| object.as_dict().ok())
        {
            resources.push(found);
        }
        // A `/Parent` pointing back at a node already walked is malformed, and
        // following it would read the same resources over and over.
        node = dictionary
            .get(b"Parent")
            .ok()
            .and_then(|object| object.as_reference().ok())
            .filter(|parent| visited.insert(*parent))
            .and_then(|parent| document.get_dictionary(parent).ok());
    }
    resources
}

/// Follows `object` through any indirect reference.
fn resolved<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Object> {
    document.dereference(object).ok().map(|(_, object)| object)
}

/// Reads an integer entry, following an indirect reference.
fn number(document: &lopdf::Document, dictionary: &lopdf::Dictionary, key: &[u8]) -> Option<i64> {
    dictionary.get_deref(key, document).ok()?.as_i64().ok()
}

/// Reads a boolean entry, following an indirect reference.
///
/// A missing or unreadable entry is false, which is the specification's
/// default for every flag read here.
fn flag(document: &lopdf::Document, dictionary: &lopdf::Dictionary, key: &[u8]) -> bool {
    dictionary
        .get_deref(key, document)
        .ok()
        .and_then(|object| object.as_bool().ok())
        .unwrap_or(false)
}

/// Reads `/Filter`, which is one name or an array of them.
///
/// Read here rather than through `lopdf::Stream::filters`, which splits the
/// same two shapes but takes each name as written: an entry given as an
/// indirect reference reads as no filter at all, and an image whose codec goes
/// unread is one this would hand to the recogniser undecoded.
fn filters_of(document: &lopdf::Document, dictionary: &lopdf::Dictionary) -> Vec<String> {
    let name = |object: &lopdf::Object| {
        object
            .as_name()
            .ok()
            .map(|name| String::from_utf8_lossy(name).into_owned())
    };

    let Some(filter) = dictionary.get_deref(b"Filter", document).ok() else {
        return Vec::new();
    };

    if let Some(single) = name(filter) {
        return vec![single];
    }
    filter
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| resolved(document, entry).and_then(name))
        .collect()
}

/// Turns one page image into bytes Leptonica opens.
fn readable(
    document: &lopdf::Document,
    image: &PageImage<'_>,
    page: u32,
    path: &Path,
) -> Result<Vec<u8>> {
    // The stream is stored as the PDF holds it, so a filter over the image
    // codec is still in the way and the bytes are not yet an image.
    if let [filter] = image.filters.as_slice() {
        if PASSTHROUGH_FILTERS.contains(&filter.as_str()) {
            return Ok(image.content.to_vec());
        }
        if filter == "CCITTFaxDecode" {
            return fax_as_tiff(document, image, page, path);
        }
    }

    bail!(
        "page {page} of {} holds an image encoded with {}, which cannot be read. \
         Only a DCTDecode, JPXDecode or CCITTFaxDecode image with nothing layered \
         over it is recognised. Export that page as an image and pass it separately.",
        path.display(),
        describe(&image.filters)
    )
}

/// Renders `filters` for an error message.
///
/// The names are written by whoever produced the document, and the message
/// travels: with the agent hooks installed it is put in front of a model. So
/// each name is bounded and stripped to printable characters rather than
/// repeated as it was found.
fn describe(filters: &[String]) -> String {
    if filters.is_empty() {
        return "no filter".to_owned();
    }
    filters
        .iter()
        .map(|filter| crate::convert::quoted(filter))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Checks a fax image can be described at all, and returns the width, height
/// and byte count the header will state.
///
/// Everything the document says about the image is taken on trust until here:
/// the coding may be one this cannot express, and the size may be one no page
/// carries and no machine should be asked to allocate.
fn fax_geometry(
    image: &PageImage<'_>,
    parameters: &FaxParameters,
    page: u32,
    path: &Path,
) -> Result<(u32, u32, u32)> {
    // Byte alignment has no equivalent in the header built here, and a stream
    // described without it decodes to noise, which would be recognised as
    // stray characters rather than failing.
    if parameters.byte_aligned {
        bail!(
            "page {page} of {} holds a fax image with EncodedByteAlign set, which cannot \
             be described to the recogniser. Reading it would recognise noise rather than \
             the page.",
            path.display()
        );
    }

    // The stream is coded against Columns, so that is the width to describe it
    // by. Falling back to the image's own width rather than the 1728 the
    // specification defaults to: a writer omitting Columns for a page that is
    // not fax-width meant the page's width.
    let stated_columns = parameters.columns.unwrap_or(image.width);
    let stated_rows = parameters.rows.unwrap_or(image.height);

    // The recogniser allocates from what the header says, so an image claiming
    // more than a page could hold is refused before it is described rather
    // than after the memory has gone.
    if stated_columns.max(0).saturating_mul(stated_rows.max(0)) > MAX_SCAN_PIXELS {
        bail!(
            "page {page} of {} holds a fax image claiming {stated_columns}x{stated_rows} \
             pixels, more than any page carries. Reading it would cost far more memory \
             than the file could justify.",
            path.display()
        );
    }

    let impossible =
        |what: &str| format!("page {page} of {} has an impossible {what}", path.display());
    Ok((
        u32::try_from(stated_columns).with_context(|| impossible("width"))?,
        u32::try_from(stated_rows).with_context(|| impossible("height"))?,
        u32::try_from(image.content.len()).with_context(|| impossible("image size"))?,
    ))
}

/// Wraps an undecoded CCITT fax stream in a TIFF header.
///
/// TIFF carries Group 3 and Group 4 natively, so this rewraps rather than
/// decodes: the fax bytes are copied across untouched and only the header
/// describing them is built here.
fn fax_as_tiff(
    document: &lopdf::Document,
    image: &PageImage<'_>,
    page: u32,
    path: &Path,
) -> Result<Vec<u8>> {
    /// TIFF field types, of which only these two are needed.
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    /// Where the fax data sits, immediately after the eight-byte header.
    const DATA_OFFSET: u32 = 8;

    let parameters = fax_parameters(document, image);
    let (columns, rows, bytes) = fax_geometry(image, &parameters, page, path)?;

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
    let directory = DATA_OFFSET.checked_add(bytes).with_context(|| {
        format!(
            "page {page} of {} has an impossible image size",
            path.display()
        )
    })?;
    tiff.extend_from_slice(&directory.to_le_bytes());
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
fn field(tag: u16, kind: u16, value: u32) -> [u8; 12] {
    let mut entry = [0u8; 12];
    entry[0..2].copy_from_slice(&tag.to_le_bytes());
    entry[2..4].copy_from_slice(&kind.to_le_bytes());
    entry[4..8].copy_from_slice(&1u32.to_le_bytes());
    entry[8..12].copy_from_slice(&value.to_le_bytes());
    entry
}

/// What `/DecodeParms` says about a fax stream.
struct FaxParameters {
    /// Which coding was used: negative for Group 4, zero for one-dimensional
    /// Group 3, positive for mixed Group 3.
    k: i64,
    /// Whether a one bit means black. The default is that a zero does.
    black_is_1: bool,
    /// Whether each row starts on a byte boundary.
    byte_aligned: bool,
    /// The width the stream was coded against, when it is stated and usable.
    columns: Option<i64>,
    /// The height, likewise. Zero is the specification's own default, and
    /// writers state it rather than leaving the entry out, so it is treated as
    /// absent rather than as an image of no rows.
    rows: Option<i64>,
}

/// Reads `/DecodeParms`, falling back to the defaults the specification gives.
///
/// Every entry is optional and a missing one is not an error, so this reports
/// no failure: a stream whose parameters are all absent still decodes on the
/// defaults.
fn fax_parameters(document: &lopdf::Document, image: &PageImage<'_>) -> FaxParameters {
    let parameters = image
        .dictionary
        .get(b"DecodeParms")
        .or_else(|_| image.dictionary.get(b"DP"))
        .ok()
        .and_then(|object| fax_dictionary(document, object));

    let Some(parameters) = parameters else {
        return FaxParameters {
            k: 0,
            black_is_1: false,
            byte_aligned: false,
            columns: None,
            rows: None,
        };
    };

    FaxParameters {
        k: number(document, parameters, b"K").unwrap_or(0),
        black_is_1: flag(document, parameters, b"BlackIs1"),
        byte_aligned: flag(document, parameters, b"EncodedByteAlign"),
        columns: number(document, parameters, b"Columns").filter(|columns| *columns > 0),
        rows: number(document, parameters, b"Rows").filter(|rows| *rows > 0),
    }
}

/// Finds the fax parameters among whatever `/DecodeParms` turned out to be.
///
/// It may be written as one dictionary or as an array matching `/Filter`, and
/// either it or its entries may be an indirect reference like any other
/// object. Read unresolved, a reference silently loses every parameter.
fn fax_dictionary<'a>(
    document: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Option<&'a lopdf::Dictionary> {
    let (_, object) = document.dereference(object).ok()?;
    if let Ok(dictionary) = object.as_dict() {
        return Some(dictionary);
    }
    object.as_array().ok()?.iter().find_map(|entry| {
        let (_, entry) = document.dereference(entry).ok()?;
        entry.as_dict().ok()
    })
}
