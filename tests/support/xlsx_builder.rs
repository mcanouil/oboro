//! Minimal OOXML workbook builder for tests.
//!
//! One copy serves both the `convert::xlsx` unit tests (pulled in with a
//! `#[path]` include) and the integration-test binaries (through
//! `tests/support/mod.rs`), so the fiddly XML cannot drift between them.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

/// Writes a minimal xlsx with inline-string cells, one entry per sheet.
///
/// The same OOXML shape as the committed `clients.xlsx` fixture, so calamine
/// is known to read it.
pub fn write_xlsx(path: &Path, sheets: &[(&str, &[&[&str]])]) {
    let file = std::fs::File::create(path).expect("creating the workbook file");
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    archive
        .start_file("[Content_Types].xml", options)
        .expect("starting a member");
    archive
        .write_all(
            br#"<?xml version="1.0"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
</Types>"#,
        )
        .expect("writing content types");

    archive
        .start_file("_rels/.rels", options)
        .expect("starting a member");
    archive
        .write_all(
            br#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        )
        .expect("writing package relationships");

    let mut workbook_sheets = String::new();
    let mut workbook_rels = String::new();
    for (index, (name, _)) in sheets.iter().enumerate() {
        let id = index + 1;
        write!(
            workbook_sheets,
            r#"<sheet name="{name}" sheetId="{id}" r:id="rId{id}"/>"#
        )
        .expect("formatting");
        write!(
            workbook_rels,
            r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{id}.xml"/>"#
        )
        .expect("formatting");
    }

    archive
        .start_file("xl/workbook.xml", options)
        .expect("starting a member");
    archive
        .write_all(
            format!(
                r#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets>{workbook_sheets}</sheets>
</workbook>"#
            )
            .as_bytes(),
        )
        .expect("writing the workbook part");

    archive
        .start_file("xl/_rels/workbook.xml.rels", options)
        .expect("starting a member");
    archive
        .write_all(
            format!(
                r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
{workbook_rels}
</Relationships>"#
            )
            .as_bytes(),
        )
        .expect("writing workbook relationships");

    for (index, (_, rows)) in sheets.iter().enumerate() {
        archive
            .start_file(format!("xl/worksheets/sheet{}.xml", index + 1), options)
            .expect("starting a member");
        archive
            .write_all(worksheet_xml(rows).as_bytes())
            .expect("writing a worksheet part");
    }

    archive.finish().expect("finishing the archive");
}

/// A worksheet part holding `rows` as inline-string cells.
fn worksheet_xml(rows: &[&[&str]]) -> String {
    let mut body = String::new();
    for (row_index, row) in rows.iter().enumerate() {
        write!(body, "<row r=\"{}\">", row_index + 1).expect("formatting");
        for (column, cell) in row.iter().enumerate() {
            let reference = format!(
                "{}{}",
                char::from(b'A' + u8::try_from(column).expect("few columns")),
                row_index + 1
            );
            write!(
                body,
                r#"<c r="{reference}" t="inlineStr"><is><t>{cell}</t></is></c>"#
            )
            .expect("formatting");
        }
        body.push_str("</row>");
    }
    format!(
        r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>{body}</sheetData>
</worksheet>"#
    )
}
