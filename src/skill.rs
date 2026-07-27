//! The skill that tells an agent what the hooks have done to what it reads.
//!
//! The hooks put placeholders in front of an agent without explaining them, and
//! `[[EMAIL_1]]` reads as a bug, a template or a redaction to route around
//! unless something says otherwise. That something is the skill, and it is
//! carried in the binary rather than fetched, so the text and the behaviour it
//! describes ship together.
//!
//! Unlike the hooks, which Oboro reports on but does not yet write, this file
//! is written into the user's `.claude` directory on request: it is inert
//! markdown in a directory Oboro creates and owns, so there is nothing to merge
//! and nothing to lose. Writing it still happens only because a flag asked for
//! it, only after the path has been named, and never through a symlink.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The skill text, compiled in so the binary and the file cannot disagree.
pub const SKILL: &str = include_str!("../skills/oboro/SKILL.md");

/// The suffix given to the copy written beside a skill someone has edited.
///
/// Overwriting an edit is not Oboro's call to make, and refusing outright would
/// leave no way forward but `--force`. The new text goes next to the old one so
/// the two can be compared and the edit kept if it is worth keeping.
pub const PROPOSED_SUFFIX: &str = ".oboro-proposed";

/// Where a skill is installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// `.claude/skills/` in the working directory, covering this project.
    Project,
    /// `~/.claude/skills/`, covering every project.
    User,
}

impl Scope {
    /// How the scope is spelled as a flag, for messages that suggest one.
    #[must_use]
    pub fn flag(self) -> &'static str {
        match self {
            Self::Project => "--project",
            Self::User => "--user",
        }
    }

    /// The directory the scope is measured from: the project, or the home
    /// directory. `None` when the user scope is asked for and there is no home.
    #[must_use]
    pub fn root(self, cwd: &Path) -> Option<PathBuf> {
        match self {
            Self::Project => Some(cwd.to_path_buf()),
            Self::User => dirs::home_dir(),
        }
    }

    /// The skill file itself, or `None` when the user has no home.
    #[must_use]
    pub fn path(self, cwd: &Path) -> Option<PathBuf> {
        self.root(cwd)
            .map(|root| root.join(SKILL_PATH.iter().collect::<PathBuf>()))
    }
}

/// Where the skill sits below a scope's root.
const SKILL_PATH: [&str; 4] = [".claude", "skills", "oboro", "SKILL.md"];

/// What is at a scope's path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Nothing is installed there.
    Missing,
    /// The installed text is the text this binary carries.
    Current,
    /// Something else is there: an older release, or a hand-edited copy.
    Edited,
    /// The path exists but could not be read.
    Unreadable,
}

/// What `install` did, so the caller can report it without guessing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Installed {
    /// The file was written.
    Written,
    /// The file already held this text, so nothing was written.
    AlreadyCurrent,
    /// An edited file was left alone and this proposal written beside it.
    Proposed(PathBuf),
}

/// What is installed at `scope`.
#[must_use]
pub fn status(scope: Scope, cwd: &Path) -> Status {
    let Some(path) = scope.path(cwd) else {
        return Status::Missing;
    };
    if !path.exists() {
        return Status::Missing;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) if text == SKILL => Status::Current,
        Ok(_) => Status::Edited,
        Err(_) => Status::Unreadable,
    }
}

/// Writes the skill to `scope`, leaving an edited file alone unless `force`.
///
/// # Errors
///
/// Returns an error when the scope has no path, when any part of the target is
/// a symbolic link, or when the directory or file cannot be written.
pub fn install(scope: Scope, cwd: &Path, force: bool) -> Result<Installed> {
    let root = scope.root(cwd).context(NO_HOME)?;
    let path = root.join(SKILL_PATH.iter().collect::<PathBuf>());
    refuse_symlinks(&root)?;

    if !force {
        match status(scope, cwd) {
            Status::Current => return Ok(Installed::AlreadyCurrent),
            Status::Edited | Status::Unreadable => {
                let proposed = proposed_path(&path);
                write(&proposed)?;
                return Ok(Installed::Proposed(proposed));
            }
            Status::Missing => {}
        }
    }

    write(&path)?;
    Ok(Installed::Written)
}

/// The path a scope resolves to, or an error naming why it does not.
///
/// # Errors
///
/// Returns an error when the user scope is asked for and no home directory can
/// be found.
pub fn path_for(scope: Scope, cwd: &Path) -> Result<PathBuf> {
    scope.path(cwd).context(NO_HOME)
}

/// Said when the user scope is asked for on a machine with no home directory.
const NO_HOME: &str = "finding your home directory to install the skill for every project; \
                       install it for this project alone with --project";

/// Where the proposal goes when an edited file is left in place.
#[must_use]
pub fn proposed_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(PROPOSED_SUFFIX);
    PathBuf::from(name)
}

/// Writes `SKILL` to `path`, creating the directories above it.
fn write(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, SKILL).with_context(|| format!("writing {}", path.display()))
}

/// Refuses to write through a symbolic link anywhere below the scope's root.
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
fn refuse_symlinks(root: &Path) -> Result<()> {
    let mut current = root.to_path_buf();

    for component in SKILL_PATH {
        current.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            // Nothing there yet, so nothing below it exists to be a link.
            return Ok(());
        };
        if metadata.file_type().is_symlink() {
            bail!(
                "{} is a symbolic link, and the skill is not written through one; \
                 remove it, or install into the other scope",
                current.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The skill describes the placeholder shape the vault issues. Spelling it
    /// out by hand here would let the code change while the skill went on
    /// describing the old shape, which is the drift that matters: the text is
    /// only worth shipping while it is true.
    #[test]
    fn the_skill_shows_the_placeholder_shape_the_vault_issues() {
        let example = crate::vault::placeholder("EMAIL", 1);
        assert!(
            SKILL.contains(&example),
            "the skill must show {example}, the placeholder the vault issues"
        );
    }

    /// Same reason, for the events the skill tells an agent to expect. An event
    /// renamed in `hooks` and left unrenamed here would teach an agent to look
    /// for something that no longer fires.
    #[test]
    fn the_skill_names_every_hook_event() {
        for (event, _) in crate::hooks::EVENTS {
            assert!(
                SKILL.contains(event),
                "the skill must name the {event} hook it describes"
            );
        }
    }

    #[test]
    fn a_missing_skill_is_missing_and_installing_writes_it() {
        let home = tempfile::tempdir().expect("temporary directory");
        assert_eq!(status(Scope::Project, home.path()), Status::Missing);

        let outcome = install(Scope::Project, home.path(), false).expect("installing");

        assert_eq!(outcome, Installed::Written);
        assert_eq!(status(Scope::Project, home.path()), Status::Current);
        let written = std::fs::read_to_string(Scope::Project.path(home.path()).expect("a path"))
            .expect("reading it back");
        assert_eq!(written, SKILL);
    }

    #[test]
    fn installing_twice_writes_nothing_the_second_time() {
        let home = tempfile::tempdir().expect("temporary directory");
        install(Scope::Project, home.path(), false).expect("installing");

        let outcome = install(Scope::Project, home.path(), false).expect("installing again");

        assert_eq!(outcome, Installed::AlreadyCurrent);
    }

    #[test]
    fn an_edited_skill_is_left_alone_and_a_proposal_written_beside_it() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Scope::Project.path(home.path()).expect("a path");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the directory");
        std::fs::write(&path, "mine, edited").expect("writing an edited skill");

        let outcome = install(Scope::Project, home.path(), false).expect("installing");

        assert_eq!(outcome, Installed::Proposed(proposed_path(&path)));
        assert_eq!(
            std::fs::read_to_string(&path).expect("reading the edited skill"),
            "mine, edited",
            "the edited skill is untouched"
        );
        assert_eq!(
            std::fs::read_to_string(proposed_path(&path)).expect("reading the proposal"),
            SKILL
        );
    }

    #[test]
    fn forcing_overwrites_an_edited_skill_and_proposes_nothing() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Scope::Project.path(home.path()).expect("a path");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the directory");
        std::fs::write(&path, "mine, edited").expect("writing an edited skill");

        let outcome = install(Scope::Project, home.path(), true).expect("installing");

        assert_eq!(outcome, Installed::Written);
        assert_eq!(status(Scope::Project, home.path()), Status::Current);
        assert!(
            !proposed_path(&path).exists(),
            "forcing writes the skill itself, not a proposal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_skill_is_refused_rather_than_written_through() {
        let home = tempfile::tempdir().expect("temporary directory");
        let elsewhere = home.path().join("elsewhere.md");
        std::fs::write(&elsewhere, "not the skill").expect("writing the link target");
        let path = Scope::Project.path(home.path()).expect("a path");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the directory");
        std::os::unix::fs::symlink(&elsewhere, &path).expect("linking");

        let error = install(Scope::Project, home.path(), true).expect_err("must refuse");

        assert!(format!("{error:#}").contains("symbolic link"));
        assert_eq!(
            std::fs::read_to_string(&elsewhere).expect("reading the link target"),
            "not the skill",
            "the link target is untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_above_the_skill_is_refused_too() {
        let home = tempfile::tempdir().expect("temporary directory");
        let elsewhere = home.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("creating the link target");
        std::fs::create_dir_all(home.path().join(".claude")).expect("creating .claude");
        std::os::unix::fs::symlink(&elsewhere, home.path().join(".claude/skills"))
            .expect("linking");

        let error = install(Scope::Project, home.path(), true).expect_err("must refuse");

        assert!(format!("{error:#}").contains("symbolic link"));
        assert!(
            !elsewhere.join("oboro/SKILL.md").exists(),
            "nothing is written through the link"
        );
    }
}
