//! `oboro` is an anonymisation layer between raw files and large language
//! models.
//!
//! It converts a document to text, replaces sensitive values with stable
//! placeholders, and keeps the mapping in a local encrypted vault so a
//! model's answer can be restored afterwards. Nothing leaves the machine.

/// The implementation of [`note!`]. Not for use on its own; it is public only
/// because the binary is a separate crate.
///
/// The line and its newline are written in one call: standard error is
/// unbuffered, and `writeln!` would write the two separately, leaving a line
/// with no newline behind when the reader goes away in between.
#[doc(hidden)]
pub fn note_args(args: std::fmt::Arguments<'_>) {
    use std::io::Write as _;
    let line = format!("{args}\n");
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

/// Writes one diagnostic line to standard error, taking the same arguments as
/// `eprintln!` but stopping quietly when the reader is gone.
///
/// `eprintln!` panics when the write fails, so `oboro clean notes/ 2>&1 |
/// head -n 1` ended in a crash report rather than stopping, and a command that
/// failed exited 101 instead of 1. Standard error is where a failure would be
/// reported, so a failure to write to it has nowhere left to go: the line is
/// dropped, and what the run did, its exit code and the files it wrote, is
/// unchanged.
///
/// Standard output is written through `print_stdout` in the binary, which
/// deliberately decides otherwise: it swallows a closed pipe alone and reports
/// every other failure, cleaned text being the point of the run rather than a
/// remark about it.
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {
        $crate::note_args(format_args!($($arg)*))
    };
}

pub mod claude;
pub mod config;
pub mod convert;
pub mod detect;
pub mod hooks;
pub mod mcp;
#[cfg(feature = "ner")]
pub mod models;
pub mod pipeline;
pub mod review;
pub mod skill;
pub mod vault;
pub mod walk;
