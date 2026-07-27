//! `oboro` is an anonymisation layer between raw files and large language
//! models.
//!
//! It converts a document to text, replaces sensitive values with stable
//! placeholders, and keeps the mapping in a local encrypted vault so a
//! model's answer can be restored afterwards. Nothing leaves the machine.

/// Writes one diagnostic line to standard error, dropping a write that fails.
///
/// The implementation of [`note!`]; call that instead.
///
/// `eprintln!` panics when the write fails, so `oboro clean notes/ 2>&1 |
/// head -n 1` ended in a crash report rather than stopping, and a command that
/// failed exited 101 instead of 1. Standard error is where a failure would be
/// reported, so a failure to write to it has nowhere left to go: the line is
/// dropped, and what the run did, its exit code and the files it wrote, is
/// unchanged.
/// The line and its newline are written in one call: standard error is
/// unbuffered, and `writeln!` would write the two separately, leaving a line
/// with no newline behind when the reader goes away in between.
pub fn note_args(args: std::fmt::Arguments<'_>) {
    use std::io::Write as _;
    let line = format!("{args}\n");
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

/// Writes one diagnostic line to standard error, taking the same arguments as
/// `eprintln!` but stopping quietly when the reader is gone. See
/// [`note_args`].
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {
        $crate::note_args(format_args!($($arg)*))
    };
}

pub mod config;
pub mod convert;
pub mod detect;
#[cfg(feature = "ner")]
pub mod models;
pub mod pipeline;
pub mod review;
pub mod vault;
pub mod walk;
