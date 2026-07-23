//! Shared harness for the integration tests.
//!
//! Every test runs against a vault in a temporary directory, so no test can
//! read or write the developer's real `~/.oboro`.
//!
//! Each test binary compiles this module separately, so helpers used by only
//! one of them would otherwise be reported as dead code.
#![allow(dead_code)]

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
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temporary directory"),
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A `oboro` invocation bound to this workspace's vault.
    ///
    /// Run from inside the workspace so configuration discovery cannot walk up
    /// into an ancestor `oboro.toml` on the developer's machine and change what
    /// a test sees.
    pub fn command(&self) -> Command {
        let mut command = Command::cargo_bin("oboro").expect("the oboro binary must build");
        command
            .current_dir(self.dir.path())
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
