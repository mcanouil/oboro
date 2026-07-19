//! Spreadsheet text extraction.
//!
//! Every sheet is flattened to tab-separated rows under a heading, which
//! keeps cells apart so neighbouring numbers cannot merge into an entity that
//! exists in neither.

use std::path::Path;

use anyhow::{Context, Result, bail};
use calamine::{Data, Reader};

pub fn to_text(path: &Path) -> Result<String> {
    let mut workbook = calamine::open_workbook_auto(path)
        .with_context(|| format!("{} is not a readable spreadsheet", path.display()))?;

    let mut text = String::new();
    for name in workbook.sheet_names() {
        let range = workbook
            .worksheet_range(&name)
            .with_context(|| format!("reading sheet '{name}' of {}", path.display()))?;

        text.push_str("## ");
        text.push_str(&name);
        text.push_str("\n\n");

        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(render).collect();
            // Skip rows that hold nothing, which spreadsheets have in bulk.
            if cells.iter().all(String::is_empty) {
                continue;
            }
            text.push_str(&cells.join("\t"));
            text.push('\n');
        }
        text.push('\n');
    }

    if text
        .lines()
        .all(|line| line.is_empty() || line.starts_with("## "))
    {
        bail!("{} contains no cell values to read", path.display());
    }
    Ok(text)
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
        let error = to_text(&path).expect_err("must reject");
        assert!(format!("{error:#}").contains("readable spreadsheet"));
    }
}
