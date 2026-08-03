//! What the two installers writing into a `.claude` directory share, and the
//! one write Oboro uses everywhere.
//!
//! Oboro puts two things in a `.claude` directory: the skill an agent reads,
//! and the hooks that make the skill worth reading. They answer the same two
//! questions before writing anything, and this module answers them once: which
//! directory a scope means, and whether the path leading to it has been pointed
//! somewhere else.
//!
//! [`write_atomic`] is here for the third question they share, how to replace a
//! file without leaving half of one behind, but it belongs to no directory in
//! particular: `restore` writes a user's document with it too.
//!
//! Nothing here decides to write. That still happens only because a flag asked
//! for it, and each installer decides what its own file should hold.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

/// Where something Oboro installs is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// The `.claude` directory in the working directory, covering this project.
    Project,
    /// The `.claude` directory at home, covering every project.
    User,
}

/// Every scope, in the order a report lists them: nearest first.
pub const SCOPES: [Scope; 2] = [Scope::Project, Scope::User];

impl Scope {
    /// The directory the scope is measured from: the project, or the home
    /// directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the user scope is asked for on a machine with no
    /// home directory.
    pub fn root(self, cwd: &Path) -> Result<PathBuf> {
        match self {
            Self::Project => Ok(cwd.to_path_buf()),
            Self::User => dirs::home_dir().context(
                "finding your home directory to install for every project; \
                 install for this project alone with --project",
            ),
        }
    }
}

/// Refuses to write through a symbolic link anywhere below a scope's root.
///
/// A repository that ships its own `.claude` directory can point any part of
/// that path somewhere else, and following it would turn an installer into a
/// way of writing text into a file the user never named. Every component Oboro
/// would create or overwrite is checked, and the first link found stops the
/// install rather than being resolved.
///
/// Only what is below the root is checked. Above it are the user's home and the
/// directories leading to their project, which they arranged themselves and
/// which Oboro is passing through rather than creating.
///
/// # Errors
///
/// Returns an error naming the first component that is a symbolic link.
pub fn refuse_symlinks(root: &Path, components: &[&str]) -> Result<()> {
    let mut current = root.to_path_buf();

    for component in components {
        current.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            // Nothing there yet, so nothing below it exists to be a link.
            return Ok(());
        };
        if metadata.file_type().is_symlink() {
            bail!(
                "{} is a symbolic link, and Oboro does not write through one; \
                 remove it, or install into the other scope",
                current.display()
            );
        }
    }
    Ok(())
}

/// Writes a file by writing a sibling temporary and renaming it into place,
/// creating the directories above it.
///
/// `restore` overwrites the user's only copy of the answer, and a hook install
/// overwrites the settings their agent runs on, so a crash partway through a
/// direct write would lose one and corrupt the other. Renaming is atomic on the
/// same filesystem, so the destination is either the old file or the whole new
/// one.
///
/// # Errors
///
/// Returns an error if the path has no usable file name, if a directory above
/// it cannot be created, or if the temporary cannot be written or renamed.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("{} has no usable file name", path.display()))?;
    let temporary = directory.join(format!(".{file_name}.oboro-{}.tmp", std::process::id()));

    let mut file = std::fs::File::create(&temporary)
        .with_context(|| format!("creating a temporary file in {}", directory.display()))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", temporary.display()))?;
    drop(file);

    std::fs::rename(&temporary, path)
        .with_context(|| format!("replacing {} with the new contents", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Making a symbolic link on Windows needs a privilege an ordinary user does
    // not have, so there is no link to refuse and the assertion below cannot
    // hold. The refusal it covers is a Unix guarantee.
    #[cfg(unix)]
    #[test]
    fn a_link_anywhere_below_the_root_is_refused() {
        let root = tempfile::tempdir().expect("temporary directory");
        std::fs::create_dir_all(root.path().join(".claude")).expect("creating .claude");
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("creating the link target");
        std::os::unix::fs::symlink(&elsewhere, root.path().join(".claude/skills"))
            .expect("linking");

        let error = refuse_symlinks(root.path(), &[".claude", "skills", "SKILL.md"])
            .expect_err("must refuse");

        assert!(format!("{error:#}").contains("symbolic link"));
    }

    #[test]
    fn a_path_that_does_not_exist_yet_is_allowed() {
        let root = tempfile::tempdir().expect("temporary directory");

        refuse_symlinks(root.path(), &[".claude", "skills", "SKILL.md"]).expect("nothing is there");
    }

    #[test]
    fn writing_replaces_the_file_and_leaves_no_temporary_behind() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, "old").expect("writing the old contents");

        write_atomic(&path, b"new").expect("writing");

        assert_eq!(
            std::fs::read_to_string(&path).expect("reading it back"),
            "new"
        );
        let left: Vec<_> = std::fs::read_dir(directory.path())
            .expect("reading the directory")
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .filter(|name| name != "settings.json")
            .collect();
        assert!(left.is_empty(), "temporary files left behind: {left:?}");
    }
}
