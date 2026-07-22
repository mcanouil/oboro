//! Expanding command-line arguments into the concrete files to process.
//!
//! `clean` and `review` accept both files and directories. A file is taken as
//! given; a directory is walked for the formats this build can read. Keeping
//! that expansion here means both commands agree on what a directory contains
//! and on which entries are quietly ignored.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::convert;

/// The suffix `output_path` gives sanitised files, excluded from walks so a
/// second run does not try to clean its own output.
const OUTPUT_SUFFIX: &str = ".clean.md";

/// A single file to process.
pub struct Input {
    pub path: PathBuf,
    /// The directory argument this file was discovered under, used to mirror
    /// the input tree into an output directory. `None` for a file named
    /// directly on the command line, which keeps the flat, beside-input
    /// behaviour.
    pub root: Option<PathBuf>,
}

/// The outcome of expanding the arguments.
pub struct Resolved {
    pub inputs: Vec<Input>,
    /// Files skipped during a directory walk because no format matched. Named
    /// files with an unsupported type are not counted here: they are kept so
    /// the conversion step still reports them as an error.
    pub skipped: usize,
}

/// Expands `args` into the files to process.
///
/// A file argument is kept as-is. A directory argument is walked, top level
/// only unless `recursive`, skipping hidden entries, symlinks, and existing
/// `*.clean.md` outputs, and counting unreadable formats as skipped.
///
/// # Errors
///
/// Returns an error if an argument cannot be inspected or a directory cannot
/// be read.
pub fn resolve(args: &[PathBuf], recursive: bool) -> Result<Resolved> {
    let mut resolved = Resolved {
        inputs: Vec::new(),
        skipped: 0,
    };
    for arg in args {
        let metadata =
            std::fs::metadata(arg).with_context(|| format!("inspecting {}", arg.display()))?;
        if metadata.is_dir() {
            walk(arg, arg, recursive, &mut resolved)?;
        } else {
            resolved.inputs.push(Input {
                path: arg.clone(),
                root: None,
            });
        }
    }
    Ok(resolved)
}

/// Walks `dir`, attributing every file found to `root` for output mirroring.
fn walk(dir: &Path, root: &Path, recursive: bool, resolved: &mut Resolved) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("reading directory {}", dir.display()))?;
    // read_dir order is unspecified; sort so a run's output ordering is stable.
    entries.sort();

    for path in entries {
        // A symlink is not followed: it could point outside the tree or form a
        // loop, and it is never something the user meant to hand over.
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspecting {}", path.display()))?;
        if metadata.is_symlink() {
            continue;
        }
        if is_hidden(&path) {
            continue;
        }
        if metadata.is_dir() {
            if recursive {
                walk(&path, root, recursive, resolved)?;
            }
            continue;
        }
        if ends_with_output_suffix(&path) {
            continue;
        }
        if convert::format_of(&path).is_some() {
            resolved.inputs.push(Input {
                path,
                root: Some(root.to_path_buf()),
            });
        } else {
            resolved.skipped += 1;
        }
    }
    Ok(())
}

/// Whether the final path component starts with a dot.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

/// Whether the file name ends with the sanitised-output suffix.
fn ends_with_output_suffix(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(OUTPUT_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("creating parent");
        }
        std::fs::write(path, contents).expect("writing");
    }

    /// A tree with one of every kind the walk has to reason about.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temporary directory");
        let root = dir.path();
        write(&root.join("note.txt"), "top");
        write(&root.join("report.md"), "top");
        write(&root.join("archive.zip"), "binary");
        write(&root.join("already.clean.md"), "output");
        write(&root.join(".secret.txt"), "hidden");
        write(&root.join("sub/nested.txt"), "deep");
        write(&root.join(".hidden/buried.txt"), "in hidden dir");
        dir
    }

    fn names(resolved: &Resolved) -> Vec<String> {
        let mut names: Vec<String> = resolved
            .inputs
            .iter()
            .map(|input| {
                input
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("file name")
                    .to_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn non_recursive_keeps_only_top_level_supported_files() {
        let dir = tree();
        let resolved = resolve(&[dir.path().to_path_buf()], false).expect("resolving");
        assert_eq!(names(&resolved), vec!["note.txt", "report.md"]);
        // Only the top-level archive.zip counts; the hidden and nested trees
        // are not descended into.
        assert_eq!(resolved.skipped, 1);
    }

    #[test]
    fn recursive_descends_into_subdirectories() {
        let dir = tree();
        let resolved = resolve(&[dir.path().to_path_buf()], true).expect("resolving");
        assert_eq!(
            names(&resolved),
            vec!["nested.txt", "note.txt", "report.md"]
        );
        // The hidden directory is skipped whole, so its file is not counted.
        assert_eq!(resolved.skipped, 1);
    }

    #[test]
    fn discovered_files_carry_their_root() {
        let dir = tree();
        let resolved = resolve(&[dir.path().to_path_buf()], true).expect("resolving");
        for input in &resolved.inputs {
            assert_eq!(input.root.as_deref(), Some(dir.path()));
        }
    }

    #[test]
    fn hidden_output_and_binary_files_are_excluded() {
        let dir = tree();
        let resolved = resolve(&[dir.path().to_path_buf()], true).expect("resolving");
        let names = names(&resolved);
        assert!(!names.iter().any(|name| name == "already.clean.md"));
        assert!(!names.iter().any(|name| name == ".secret.txt"));
        assert!(!names.iter().any(|name| name == "archive.zip"));
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_are_not_followed() {
        let dir = tree();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(dir.path().join("note.txt"), &link).expect("symlink");
        let resolved = resolve(&[dir.path().to_path_buf()], false).expect("resolving");
        assert!(!names(&resolved).iter().any(|name| name == "link.txt"));
    }

    #[test]
    fn a_named_file_is_kept_with_no_root() {
        let dir = tree();
        let file = dir.path().join("note.txt");
        let resolved = resolve(std::slice::from_ref(&file), false).expect("resolving");
        assert_eq!(resolved.inputs.len(), 1);
        assert_eq!(resolved.inputs[0].path, file);
        assert_eq!(resolved.inputs[0].root, None);
        assert_eq!(resolved.skipped, 0);
    }

    #[test]
    fn a_named_unsupported_file_is_kept_for_the_converter_to_reject() {
        let dir = tree();
        let file = dir.path().join("archive.zip");
        let resolved = resolve(std::slice::from_ref(&file), false).expect("resolving");
        // Kept, not counted as skipped: conversion will raise the error.
        assert_eq!(resolved.inputs.len(), 1);
        assert_eq!(resolved.inputs[0].path, file);
        assert_eq!(resolved.skipped, 0);
    }
}
