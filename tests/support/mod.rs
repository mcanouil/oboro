//! Shared harness for the integration tests.
//!
//! Every test runs against a vault in a temporary directory, so no test can
//! read or write the developer's real `~/.hush`.
//!
//! Each test binary compiles this module separately, so helpers used by only
//! one of them would otherwise be reported as dead code.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

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

    /// A `hush` invocation bound to this workspace's vault.
    pub fn command(&self) -> Command {
        let mut command = Command::cargo_bin("hush").expect("the hush binary must build");
        command
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
            .arg(fixture("hush.toml"))
            .arg("--stdout")
            .output()
            .expect("running hush clean");
        assert!(
            output.status.success(),
            "hush clean failed: {}",
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
            .expect("running hush restore");
        assert!(
            output.status.success(),
            "hush restore failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("output must be UTF-8")
    }
}
