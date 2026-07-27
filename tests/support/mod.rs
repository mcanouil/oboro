//! Shared harness for the integration tests.
//!
//! Every test runs against a vault in a temporary directory, so no test can
//! read or write the developer's real `~/.oboro`.
//!
//! Each test binary compiles this module separately, so helpers used by only
//! one of them would otherwise be reported as dead code, and re-exports it
//! does not reach for as unused imports.
#![allow(dead_code, unused_imports)]

mod xlsx_builder;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

pub use xlsx_builder::write_xlsx;

/// Absolute path to a file in `testdata/`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name)
}

pub struct Workspace {
    dir: TempDir,
    /// A directory of its own rather than one inside `dir`: the tests that
    /// assert nothing was written beside an invocation read `dir` and would
    /// count a home directory as a leaked file.
    home: TempDir,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temporary directory"),
            home: tempfile::tempdir().expect("temporary home"),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The store this workspace's invocations use, so a test asserting that
    /// nothing was written can tell the store apart from a leaked file.
    pub fn store_paths(&self) -> [PathBuf; 2] {
        [
            self.dir.path().join("vault.db"),
            self.dir.path().join("key"),
        ]
    }

    /// The home directory this workspace's invocations see, so a command that
    /// writes for every user writes here rather than into the developer's own
    /// `~`.
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// A `oboro` invocation bound to this workspace's vault.
    ///
    /// Run from inside the workspace so configuration discovery cannot walk up
    /// into an ancestor `oboro.toml` on the developer's machine and change what
    /// a test sees.
    ///
    /// The locale is fixed too: without a `regions` key the phone recogniser
    /// takes its hint from the environment, so the developer's own locale would
    /// otherwise decide what these tests detect.
    pub fn command(&self) -> Command {
        self.command_in_locale("fr_FR.UTF-8")
    }

    /// A `oboro` invocation running in `locale`, for the tests that care.
    pub fn command_in_locale(&self, locale: &str) -> Command {
        Command::from_std(self.std_command(locale))
    }

    /// The same invocation as [`Workspace::command_in_locale`], as a plain
    /// [`std::process::Command`], for the tests that have to spawn the process
    /// themselves and drive its pipes.
    pub fn std_command(&self, locale: &str) -> std::process::Command {
        let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin("oboro"));
        command
            .current_dir(self.dir.path())
            .env("HOME", self.home())
            .env("LC_ALL", locale)
            .env("LANG", locale)
            .arg("--vault")
            .arg(self.dir.path().join("vault.db"))
            .arg("--key")
            .arg(self.dir.path().join("key"));
        command
    }

    /// Cleans a fixture with the fixture configuration, returning the output.
    pub fn clean_fixture(&self, name: &str) -> String {
        let output = self
            .command()
            .arg("clean")
            .arg(fixture(name))
            .arg("--config")
            .arg(fixture("oboro.toml"))
            .arg("--stdout")
            .output()
            .expect("running oboro clean");
        assert!(
            output.status.success(),
            "oboro clean failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("output must be UTF-8")
    }

    /// Cleans `text` piped in on standard input, with the fixture
    /// configuration, returning the output.
    ///
    /// The counterpart to [`Workspace::clean_fixture`] for the path an agent
    /// hook takes, where the document is in memory and never touches disk.
    pub fn clean_piped(&self, text: &str) -> String {
        let output = self
            .command()
            .arg("clean")
            .arg("-")
            .arg("--config")
            .arg(fixture("oboro.toml"))
            .write_stdin(text.to_owned())
            .output()
            .expect("running oboro clean");
        assert!(
            output.status.success(),
            "oboro clean failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("output must be UTF-8")
    }

    /// Restores placeholders in `text` piped in on standard input.
    pub fn restore_piped(&self, text: &str) -> String {
        let output = self
            .command()
            .arg("restore")
            .arg("-")
            .write_stdin(text.to_owned())
            .output()
            .expect("running oboro restore");
        assert!(
            output.status.success(),
            "oboro restore failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("output must be UTF-8")
    }

    /// Restores placeholders in `text` using this workspace's vault.
    pub fn restore(&self, text: &str) -> String {
        let path = self.dir.path().join("answer.md");
        std::fs::write(&path, text).expect("writing the answer file");
        let output = self
            .command()
            .arg("restore")
            .arg(&path)
            .arg("--stdout")
            .output()
            .expect("running oboro restore");
        assert!(
            output.status.success(),
            "oboro restore failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("output must be UTF-8")
    }
}
