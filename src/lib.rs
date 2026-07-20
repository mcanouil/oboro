//! `oboro` is an anonymisation layer between raw files and large language
//! models.
//!
//! It converts a document to text, replaces sensitive values with stable
//! placeholders, and keeps the mapping in a local encrypted vault so a
//! model's answer can be restored afterwards. Nothing leaves the machine.

pub mod config;
pub mod convert;
pub mod detect;
#[cfg(feature = "ner")]
pub mod models;
pub mod pipeline;
pub mod review;
pub mod vault;
