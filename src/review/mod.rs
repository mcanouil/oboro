//! Reviewing detections before anything is written.
//!
//! The recognition model errs towards redacting, so a document comes back
//! with more holes than it needs. This is where the user puts some of them
//! back.
//!
//! The decision logic lives here and the terminal drawing lives in [`ui`], so
//! what the tool does can be tested without a terminal.

mod ui;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::detect::{Detector, Span};
use crate::vault::Vault;

pub use ui::run;

/// One detection, and whether the user wants it redacted.
pub struct Decision {
    pub span: Span,
    /// Accepted detections are replaced; rejected ones are left as they are.
    pub accepted: bool,
}

/// A document under review.
pub struct Document {
    pub path: PathBuf,
    /// The directory the file was discovered under, mirrored into the output
    /// directory on write. `None` for a file named directly on the command
    /// line. See [`crate::walk::Input`].
    pub root: Option<PathBuf>,
    /// The workbook sheet this document came from, as its zero-based position
    /// and raw name. `None` for a whole-file document. The name may hold PII
    /// and path-hostile characters; [`SheetNamer`] turns it into a filename
    /// part.
    pub sheet: Option<(usize, String)>,
    pub text: String,
    pub decisions: Vec<Decision>,
}

impl Document {
    /// Reads and analyses `path` with a detector shared across the batch.
    ///
    /// A plain document yields one entry; a workbook yields one per sheet, so
    /// each table is reviewed and written on its own.
    ///
    /// `root` is the directory the file was discovered under, used to mirror
    /// the tree on write; it is `None` for a directly named file.
    ///
    /// Every detection starts accepted, so confirming without touching
    /// anything gives exactly what `clean` would have produced.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or detection fails.
    pub fn open_all(path: &Path, root: Option<&Path>, detector: &Detector) -> Result<Vec<Self>> {
        crate::convert::read(path)?
            .into_parts()
            .into_iter()
            .map(|(sheet, text)| {
                let decisions = crate::pipeline::detect(&text, detector)?
                    .into_iter()
                    .map(|span| Decision {
                        span,
                        accepted: true,
                    })
                    .collect();
                Ok(Self {
                    path: path.to_path_buf(),
                    root: root.map(Path::to_path_buf),
                    sheet,
                    text,
                    decisions,
                })
            })
            .collect()
    }

    /// Names this document for messages: the path, plus the sheet when it is
    /// one table of a workbook.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.sheet {
            Some((_, name)) => format!("{} [{name}]", self.path.display()),
            None => self.path.display().to_string(),
        }
    }

    #[must_use]
    pub fn accepted_count(&self) -> usize {
        self.decisions
            .iter()
            .filter(|decision| decision.accepted)
            .count()
    }

    /// The line the span sits on, for showing it in context.
    ///
    /// Seeing "Bernard" alone says nothing about whether it is a surname or a
    /// street; the line it came from usually settles it.
    #[must_use]
    pub fn context(&self, index: usize) -> &str {
        let Some(decision) = self.decisions.get(index) else {
            return "";
        };
        let start = self.text[..decision.span.start]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let end = self.text[decision.span.end..]
            .find('\n')
            .map_or(self.text.len(), |newline| decision.span.end + newline);
        self.text[start..end].trim()
    }

    /// Applies the accepted decisions, leaving the rest of the text alone.
    ///
    /// # Errors
    ///
    /// Returns an error if the vault cannot allocate a placeholder.
    pub fn apply(&self, vault: &mut Vault) -> Result<crate::pipeline::CleanReport> {
        let accepted: Vec<Span> = self
            .decisions
            .iter()
            .filter(|decision| decision.accepted)
            .map(|decision| decision.span.clone())
            .collect();
        crate::pipeline::apply(&self.text, &accepted, vault)
    }

    /// Writes the reviewed result beside the input, as `clean` does.
    ///
    /// `written` records every destination this run has produced; a second
    /// document resolving to one of them is refused before its values are
    /// stored, so an earlier reviewed output is never silently replaced.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination was already written this run, the
    /// vault fails, or the file cannot be written.
    pub fn write(
        &self,
        vault: &mut Vault,
        output_dir: Option<&Path>,
        stem_override: Option<&str>,
        sheet_fragment: Option<&str>,
        written: &mut Vec<PathBuf>,
    ) -> Result<PathBuf> {
        let destination = output_path(
            &self.path,
            output_dir,
            self.root.as_deref(),
            stem_override,
            sheet_fragment,
        )?;
        if written.contains(&destination) {
            bail!(
                "two inputs would both be written to {}; review them separately \
                 or into different output directories",
                destination.display()
            );
        }
        let report = self.apply(vault)?;
        write_output(&destination, &report.text)?;
        written.push(destination.clone());
        Ok(destination)
    }
}

/// Writes sanitised `text` at `destination`, creating parent directories.
///
/// The one write path for `clean` and `review`, so the two commands cannot
/// drift in how outputs land on disk.
///
/// # Errors
///
/// Returns an error if a directory or the file cannot be created.
pub fn write_output(destination: &Path, text: &str) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    std::fs::write(destination, text).with_context(|| format!("writing {}", destination.display()))
}

/// Builds the sanitised output path, the suffix following the input's format:
/// `report.docx` becomes `report.clean.md`, `data.csv` becomes
/// `data.clean.csv`, and `book.xlsx` with the sheet fragment `Clients`
/// becomes `book.Clients.clean.tsv`.
///
/// When `output_dir` and `root` are both given, the input's location relative
/// to `root` is mirrored under `output_dir`, so files sharing a name in
/// different subdirectories do not collide. Otherwise the file lands directly
/// in `output_dir`, or beside the input when there is none.
///
/// `stem_override` supplies a redacted stem in place of the input's own, so PII
/// in the filename does not survive into the output name; `None` keeps the
/// input stem verbatim. `sheet_fragment` names the workbook sheet this output
/// holds, from [`SheetNamer`]; `None` for whole-file outputs.
///
/// # Errors
///
/// Returns an error if the input has no usable file name, or is not below the
/// root it was discovered under.
pub fn output_path(
    input: &Path,
    output_dir: Option<&Path>,
    root: Option<&Path>,
    stem_override: Option<&str>,
    sheet_fragment: Option<&str>,
) -> Result<PathBuf> {
    let stem = match stem_override {
        Some(stem) => stem,
        None => file_stem_str(input)?,
    };
    let suffix = crate::convert::output_suffix(crate::convert::format_of(input));
    let name = match sheet_fragment {
        Some(fragment) => format!("{stem}.{fragment}{suffix}"),
        None => format!("{stem}{suffix}"),
    };
    Ok(match (output_dir, root) {
        (Some(dir), Some(root)) => {
            let relative = input
                .strip_prefix(root)
                .with_context(|| format!("{} is not below {}", input.display(), root.display()))?;
            dir.join(relative).with_file_name(name)
        }
        (Some(dir), None) => dir.join(name),
        (None, _) => input.with_file_name(name),
    })
}

/// Redacts PII in `input`'s file stem, ready to pass to [`output_path`].
///
/// Sharing `vault` with the document's own redaction keeps placeholders
/// consistent between the name and the body.
///
/// # Errors
///
/// Returns an error if the input has no usable file name, or detection or the
/// vault fails.
pub fn redacted_stem(input: &Path, detector: &Detector, vault: &mut Vault) -> Result<String> {
    crate::pipeline::clean_stem(file_stem_str(input)?, detector, vault)
}

/// Turns a sheet name into a filesystem-safe filename fragment.
///
/// Path separators, characters some filesystems refuse, and control
/// characters become underscores; surrounding whitespace and leading dots are
/// dropped. A name with nothing usable left falls back to `sheet<N>` from
/// `index`, the sheet's zero-based position in the workbook.
#[must_use]
pub fn sheet_fragment(name: &str, index: usize) -> String {
    let sanitised: String = name
        .chars()
        .map(|character| {
            let hostile = matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '[' | ']'
            );
            if hostile || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = sanitised.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        format!("sheet{}", index + 1)
    } else {
        trimmed.to_owned()
    }
}

/// Allocates unique output-name fragments for the sheets of one workbook.
///
/// The namer remembers every fragment it hands out, so uniqueness is its own
/// property rather than bookkeeping each caller must carry correctly.
#[derive(Default)]
pub struct SheetNamer {
    used: std::collections::HashSet<String>,
}

impl SheetNamer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the output-name fragment for one sheet.
    ///
    /// When `redact` is set the name first goes through the same PII
    /// redaction as file stems, sharing `vault` so a client name in a tab and
    /// in the body get one placeholder. The result is then sanitised by
    /// [`sheet_fragment`] and made unique within this workbook by appending
    /// `_2`, `_3`… on collision.
    ///
    /// # Errors
    ///
    /// Returns an error if detection or the vault fails while redacting.
    pub fn fragment(
        &mut self,
        name: &str,
        index: usize,
        redact: bool,
        detector: &Detector,
        vault: &mut Vault,
    ) -> Result<String> {
        let base = if redact {
            crate::pipeline::clean_stem(name, detector, vault)?
        } else {
            name.to_owned()
        };
        let fragment = sheet_fragment(&base, index);
        let mut candidate = fragment.clone();
        let mut attempt = 2;
        while !self.used.insert(candidate.clone()) {
            candidate = format!("{fragment}_{attempt}");
            attempt += 1;
        }
        Ok(candidate)
    }
}

/// Extracts `input`'s file stem as UTF-8, the part reused for the output name.
fn file_stem_str(input: &Path) -> Result<&str> {
    input
        .file_stem()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no usable file name", input.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        // Rules-only, so the decisions under test do not depend on whether the
        // recognition model happens to be installed.
        let mut config = crate::config::Config::default();
        config.ner_enabled = false;
        let detector = Detector::new(&config).expect("detector");
        let decisions = crate::pipeline::detect(text, &detector)
            .expect("detecting")
            .into_iter()
            .map(|span| Decision {
                span,
                accepted: true,
            })
            .collect();
        Document {
            path: PathBuf::from("note.txt"),
            root: None,
            sheet: None,
            text: text.to_owned(),
            decisions,
        }
    }

    fn open_vault(dir: &tempfile::TempDir) -> Vault {
        Vault::open(&dir.path().join("vault.db"), &dir.path().join("key")).expect("opening a vault")
    }

    #[test]
    fn everything_starts_accepted_so_confirming_matches_clean() {
        let doc = document("Call 06 12 34 56 78 or mail a@example.com.");
        assert_eq!(doc.decisions.len(), 2);
        assert_eq!(doc.accepted_count(), 2);

        let dir = tempfile::tempdir().expect("temporary directory");
        let mut vault = open_vault(&dir);
        let reviewed = doc.apply(&mut vault).expect("applying");

        let dir2 = tempfile::tempdir().expect("temporary directory");
        let mut vault2 = open_vault(&dir2);
        let mut config = crate::config::Config::default();
        config.ner_enabled = false;
        let detector = Detector::new(&config).expect("detector");
        let cleaned = crate::pipeline::clean(&doc.text, &detector, &mut vault2).expect("cleaning");
        assert_eq!(reviewed.text, cleaned.text);
    }

    #[test]
    fn a_rejected_detection_is_left_in_the_text() {
        let mut doc = document("Call 06 12 34 56 78 or mail a@example.com.");
        doc.decisions[0].accepted = false;

        let dir = tempfile::tempdir().expect("temporary directory");
        let mut vault = open_vault(&dir);
        let report = doc.apply(&mut vault).expect("applying");

        assert!(
            report.text.contains("06 12 34 56 78"),
            "a rejected detection must survive: {}",
            report.text
        );
        assert!(!report.text.contains("a@example.com"));
        assert_eq!(report.replaced, 1);
    }

    #[test]
    fn rejecting_everything_leaves_the_document_untouched() {
        let mut doc = document("Call 06 12 34 56 78 or mail a@example.com.");
        for decision in &mut doc.decisions {
            decision.accepted = false;
        }

        let dir = tempfile::tempdir().expect("temporary directory");
        let mut vault = open_vault(&dir);
        let report = doc.apply(&mut vault).expect("applying");

        assert_eq!(report.text, doc.text);
        assert_eq!(report.replaced, 0);
    }

    #[test]
    fn a_rejected_value_is_never_stored_in_the_vault() {
        let mut doc = document("Mail a@example.com.");
        doc.decisions[0].accepted = false;

        let dir = tempfile::tempdir().expect("temporary directory");
        let mut vault = open_vault(&dir);
        doc.apply(&mut vault).expect("applying");

        assert!(
            vault.entries().expect("listing").is_empty(),
            "rejecting a detection must not record it"
        );
    }

    #[test]
    fn a_second_document_for_one_destination_is_refused_not_overwritten() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let out = dir.path().join("out");
        let first = document("Mail a@example.com.");
        let second = document("Mail b@example.com.");
        let mut vault = open_vault(&dir);
        let mut written = Vec::new();

        first
            .write(&mut vault, Some(&out), None, None, &mut written)
            .expect("first write");
        let error = second
            .write(&mut vault, Some(&out), None, None, &mut written)
            .expect_err("a second write to one destination must be refused");
        assert!(format!("{error:#}").contains("both be written to"));
        assert_eq!(
            vault.entries().expect("listing").len(),
            1,
            "the refused document must not allocate placeholders"
        );
    }

    #[test]
    fn context_shows_the_line_the_detection_came_from() {
        let doc = document("First line.\nCall 06 12 34 56 78 today.\nLast line.");
        assert_eq!(doc.context(0), "Call 06 12 34 56 78 today.");
    }

    #[test]
    fn context_handles_a_detection_on_the_only_line() {
        let doc = document("Call 06 12 34 56 78");
        assert_eq!(doc.context(0), "Call 06 12 34 56 78");
    }

    #[test]
    fn context_of_a_missing_index_is_empty() {
        let doc = document("nothing here");
        assert_eq!(doc.context(99), "");
    }

    #[test]
    fn output_is_named_after_the_input() {
        let path =
            output_path(Path::new("/tmp/report.docx"), None, None, None, None).expect("naming");
        assert_eq!(path, Path::new("/tmp/report.clean.md"));

        let path = output_path(
            Path::new("/tmp/report.docx"),
            Some(Path::new("/out")),
            None,
            None,
            None,
        )
        .expect("naming");
        assert_eq!(path, Path::new("/out/report.clean.md"));
    }

    #[test]
    fn tabular_inputs_keep_a_tabular_extension() {
        let path = output_path(Path::new("/tmp/data.csv"), None, None, None, None).expect("naming");
        assert_eq!(path, Path::new("/tmp/data.clean.csv"));

        let path = output_path(Path::new("/tmp/data.tsv"), None, None, None, None).expect("naming");
        assert_eq!(path, Path::new("/tmp/data.clean.tsv"));
    }

    #[test]
    fn a_sheet_fragment_lands_between_stem_and_suffix() {
        let path = output_path(
            Path::new("/tmp/book.xlsx"),
            None,
            None,
            None,
            Some("Clients"),
        )
        .expect("naming");
        assert_eq!(path, Path::new("/tmp/book.Clients.clean.tsv"));
    }

    #[test]
    fn output_uses_the_redacted_stem_when_given() {
        let path = output_path(
            Path::new("/tmp/jean-dupont.docx"),
            None,
            None,
            Some("PERSON_1"),
            None,
        )
        .expect("naming");
        assert_eq!(path, Path::new("/tmp/PERSON_1.clean.md"));
    }

    #[test]
    fn output_mirrors_the_input_tree_under_the_root() {
        let path = output_path(
            Path::new("/a/sub/report.docx"),
            Some(Path::new("/out")),
            Some(Path::new("/a")),
            None,
            None,
        )
        .expect("naming");
        assert_eq!(path, Path::new("/out/sub/report.clean.md"));
    }

    #[test]
    fn hostile_sheet_names_become_safe_fragments() {
        assert_eq!(sheet_fragment("P&L / Q1", 0), "P&L _ Q1");
        assert_eq!(sheet_fragment("a:b", 0), "a_b");
        assert_eq!(sheet_fragment("tab\tname", 0), "tab_name");
        assert_eq!(sheet_fragment("Clients", 0), "Clients");
    }

    #[test]
    fn a_sheet_name_with_nothing_usable_falls_back_to_its_position() {
        assert_eq!(sheet_fragment("...", 0), "sheet1");
        assert_eq!(sheet_fragment("  ", 2), "sheet3");
    }

    #[test]
    fn colliding_sheet_fragments_are_numbered_apart() {
        let mut config = crate::config::Config::default();
        config.ner_enabled = false;
        let detector = Detector::new(&config).expect("detector");
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut vault = open_vault(&dir);
        let mut namer = SheetNamer::new();

        let first = namer
            .fragment("a:b", 0, false, &detector, &mut vault)
            .expect("first fragment");
        let second = namer
            .fragment("a*b", 1, false, &detector, &mut vault)
            .expect("second fragment");
        assert_eq!(first, "a_b");
        assert_eq!(second, "a_b_2");
    }

    #[test]
    fn a_document_with_nothing_to_redact_reviews_cleanly() {
        let doc = document("Nothing sensitive at all here.");
        assert!(doc.decisions.is_empty());
        assert_eq!(doc.accepted_count(), 0);
    }
}
