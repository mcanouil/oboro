//! Spreadsheet text extraction.
//!
//! Every sheet becomes its own tab-separated table, so a workbook turns into
//! one TSV output per sheet instead of a flattened document. Tabs keep cells
//! apart so neighbouring numbers cannot merge into an entity that exists in
//! neither.

use std::path::Path;

use anyhow::{Context, Result, bail};
use calamine::{Data, Reader};

use super::Sheet;

// The workbook builder lives with the integration-test support code; this
// source include shares that one copy with the unit tests below.
#[cfg(test)]
#[path = "../../tests/support/xlsx_builder.rs"]
mod xlsx_builder;

/// Reads every non-empty sheet of the workbook as tab-separated rows.
///
/// Sheets holding no cell values are skipped entirely rather than producing
/// empty outputs.
pub fn to_sheets(path: &Path) -> Result<Vec<Sheet>> {
    let mut workbook = calamine::open_workbook_auto(path)
        .with_context(|| format!("{} is not a readable spreadsheet", path.display()))?;

    let mut sheets = Vec::new();
    for (index, name) in workbook.sheet_names().into_iter().enumerate() {
        let range = workbook
            .worksheet_range(&name)
            .with_context(|| format!("reading sheet '{name}' of {}", path.display()))?;

        let mut text = String::new();
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(render).collect();
            // Skip rows that hold nothing, which spreadsheets have in bulk.
            if cells.iter().all(String::is_empty) {
                continue;
            }
            text.push_str(&cells.join("\t"));
            text.push('\n');
        }
        if !text.is_empty() {
            sheets.push(Sheet { index, name, text });
        }
    }

    if sheets.is_empty() {
        bail!("{} contains no cell values to read", path.display());
    }
    Ok(sheets)
}

/// Renders a cell as the text a reader would see.
///
/// Floats that hold whole numbers print without a decimal point, so an
/// identifier stored as a number still looks like the identifier it is.
fn render(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),

        Data::Float(value) => {
            if value.fract() == 0.0 && value.abs() < 1e15 {
                format!("{value:.0}")
            } else {
                value.to_string()
            }
        }
        Data::Int(value) => value.to_string(),
        Data::Bool(value) => value.to_string(),
        Data::DateTime(value) => value.to_string(),
        Data::String(value) | Data::DateTimeIso(value) | Data::DurationIso(value) => value.clone(),
        Data::Error(error) => format!("{error:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::xlsx_builder::write_xlsx;

    #[test]
    fn each_sheet_becomes_its_own_tab_separated_table() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("book.xlsx");
        write_xlsx(
            &path,
            &[
                ("Clients", &[&["Name", "Email"], &["Jean", "a@example.com"]]),
                ("Notes", &[&["Topic"], &["Renewal"]]),
            ],
        );

        let sheets = to_sheets(&path).expect("reading");
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0].name, "Clients");
        assert_eq!(sheets[0].text, "Name\tEmail\nJean\ta@example.com\n");
        assert_eq!(sheets[1].name, "Notes");
        assert_eq!(sheets[1].text, "Topic\nRenewal\n");
        assert!(
            sheets.iter().all(|sheet| !sheet.text.contains("## ")),
            "sheet names must not leak into the content"
        );
    }

    #[test]
    fn an_empty_sheet_is_skipped_rather_than_written_empty() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("book.xlsx");
        write_xlsx(&path, &[("Blank", &[]), ("Data", &[&["value"]])]);

        let sheets = to_sheets(&path).expect("reading");
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].name, "Data");
        assert_eq!(
            sheets[0].index, 1,
            "the workbook position must count skipped empty sheets, \
             so a fallback name points at the right sheet"
        );
    }

    #[test]
    fn a_workbook_with_no_values_anywhere_is_rejected() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("book.xlsx");
        write_xlsx(&path, &[("Blank", &[]), ("AlsoBlank", &[])]);

        let error = to_sheets(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("no cell values"));
    }

    #[test]
    fn whole_numbers_keep_their_digits() {
        assert_eq!(render(&Data::Float(612_345_678.0)), "612345678");
        assert_eq!(render(&Data::Float(1234.56)), "1234.56");
        assert_eq!(render(&Data::Int(42)), "42");
    }

    #[test]
    fn empty_cells_render_as_nothing() {
        assert_eq!(render(&Data::Empty), "");
    }

    #[test]
    fn strings_pass_through_unchanged() {
        assert_eq!(
            render(&Data::String("Société Générale".to_owned())),
            "Société Générale"
        );
    }

    #[test]
    fn a_file_that_is_not_a_spreadsheet_is_reported_clearly() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.xlsx");
        std::fs::write(&path, "not a spreadsheet").expect("writing");
        let error = to_sheets(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable spreadsheet"));
    }
}
