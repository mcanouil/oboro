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

use anyhow::{Context, Result};

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
    pub text: String,
    pub decisions: Vec<Decision>,
}

impl Document {
    /// Reads and analyses `path` with a detector shared across the batch.
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
    pub fn open(path: &Path, root: Option<&Path>, detector: &Detector) -> Result<Self> {
        let text = crate::convert::to_text(path)?;
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
            text,
            decisions,
        })
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
    /// # Errors
    ///
    /// Returns an error if the vault fails or the file cannot be written.
    pub fn write(&self, vault: &mut Vault, output_dir: Option<&Path>) -> Result<PathBuf> {
        let report = self.apply(vault)?;
        let destination = output_path(&self.path, output_dir, self.root.as_deref())?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
        std::fs::write(&destination, &report.text)
            .with_context(|| format!("writing {}", destination.display()))?;
        Ok(destination)
    }
}

/// Builds the sanitised output path, `report.docx` becoming `report.clean.md`.
///
/// When `output_dir` and `root` are both given, the input's location relative
/// to `root` is mirrored under `output_dir`, so files sharing a name in
/// different subdirectories do not collide. Otherwise the file lands directly
/// in `output_dir`, or beside the input when there is none.
///
/// # Errors
///
/// Returns an error if the input has no usable file name, or is not below the
/// root it was discovered under.
pub fn output_path(
    input: &Path,
    output_dir: Option<&Path>,
    root: Option<&Path>,
) -> Result<PathBuf> {
    let stem = input
        .file_stem()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no usable file name", input.display()))?;
    let name = format!("{stem}.clean.md");
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
        let path = output_path(Path::new("/tmp/report.docx"), None, None).expect("naming");
        assert_eq!(path, Path::new("/tmp/report.clean.md"));

        let path = output_path(Path::new("/tmp/report.docx"), Some(Path::new("/out")), None)
            .expect("naming");
        assert_eq!(path, Path::new("/out/report.clean.md"));
    }

    #[test]
    fn output_mirrors_the_input_tree_under_the_root() {
        let path = output_path(
            Path::new("/a/sub/report.docx"),
            Some(Path::new("/out")),
            Some(Path::new("/a")),
        )
        .expect("naming");
        assert_eq!(path, Path::new("/out/sub/report.clean.md"));
    }

    #[test]
    fn a_document_with_nothing_to_redact_reviews_cleanly() {
        let doc = document("Nothing sensitive at all here.");
        assert!(doc.decisions.is_empty());
        assert_eq!(doc.accepted_count(), 0);
    }
}
