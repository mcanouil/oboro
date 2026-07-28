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

use anyhow::{Context, Result};

use crate::claude::{Scope, refuse_symlinks};

/// The skill text, compiled in so the binary and the file cannot disagree.
pub const SKILL: &str = include_str!("../skills/oboro/SKILL.md");

/// The skill file for `scope`.
///
/// # Errors
///
/// Returns an error for the same reason [`Scope::root`] does.
pub fn path(scope: Scope, cwd: &Path) -> Result<PathBuf> {
    Ok(scope
        .root(cwd)?
        .join(SKILL_PATH.iter().collect::<PathBuf>()))
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

/// What installing would do, decided once so that what is announced and what
/// is written cannot disagree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Plan {
    /// Write the skill at this path.
    Write(PathBuf),
    /// Leave this file alone, since it already holds the skill.
    Keep(PathBuf),
    /// Leave the first path alone, since it was edited, and write the second
    /// beside it so the two can be compared.
    ///
    /// Overwriting an edit is not Oboro's call to make, and refusing outright
    /// would leave no way forward but `--force`.
    Propose {
        installed: PathBuf,
        proposed: PathBuf,
    },
}

impl Plan {
    /// The file this plan writes, if it writes one.
    #[must_use]
    pub fn target(&self) -> Option<&Path> {
        match self {
            Self::Write(path) => Some(path),
            Self::Keep(_) => None,
            Self::Propose { proposed, .. } => Some(proposed),
        }
    }
}

/// What is installed at `path`.
///
/// `read_to_string` answers both questions at once: a missing file is reported
/// as missing rather than stat'd for separately, which also keeps this from
/// disagreeing with itself when the path changes underneath.
#[must_use]
pub fn status(path: &Path) -> Status {
    match std::fs::read_to_string(path) {
        Ok(text) if text == SKILL => Status::Current,
        Ok(_) => Status::Edited,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Status::Missing,
        Err(_) => Status::Unreadable,
    }
}

/// What installing into `scope` would do, without doing any of it.
///
/// # Errors
///
/// Returns an error when the scope has no path, or when any part of the target
/// is a symbolic link.
pub fn plan(scope: Scope, cwd: &Path, force: bool) -> Result<Plan> {
    let root = scope.root(cwd)?;
    refuse_symlinks(&root, &SKILL_PATH)?;
    let path = root.join(SKILL_PATH.iter().collect::<PathBuf>());

    if force {
        return Ok(Plan::Write(path));
    }
    Ok(match status(&path) {
        Status::Missing => Plan::Write(path),
        Status::Current => Plan::Keep(path),
        Status::Edited | Status::Unreadable => Plan::Propose {
            proposed: proposed_path(&path),
            installed: path,
        },
    })
}

/// Carries out a [`Plan`], returning it so the caller can report what happened.
///
/// # Errors
///
/// Returns an error when the directory or the file cannot be written.
pub fn install(plan: Plan) -> Result<Plan> {
    if let Some(target) = plan.target() {
        write(target)?;
    }
    Ok(plan)
}

/// Where the proposal goes when an edited file is left in place.
#[must_use]
pub fn proposed_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".oboro-proposed");
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
        for event in crate::hooks::EVENTS {
            assert!(
                SKILL.contains(event.name),
                "the skill must name the {} hook it describes",
                event.name
            );
        }
    }

    /// Installs into a temporary project, the way the command does.
    fn install_into(cwd: &Path, force: bool) -> Result<Plan> {
        install(plan(Scope::Project, cwd, force)?)
    }

    #[test]
    fn a_missing_skill_is_written() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = path(Scope::Project, home.path()).expect("a path");
        assert_eq!(status(&path), Status::Missing);

        let done = install_into(home.path(), false).expect("installing");

        assert_eq!(done, Plan::Write(path.clone()));
        assert_eq!(status(&path), Status::Current);
    }

    #[test]
    fn installing_twice_writes_nothing_the_second_time() {
        let home = tempfile::tempdir().expect("temporary directory");
        install_into(home.path(), false).expect("installing");

        let done = install_into(home.path(), false).expect("installing again");

        let path = path(Scope::Project, home.path()).expect("a path");
        assert_eq!(done, Plan::Keep(path));
        assert_eq!(done.target(), None, "keeping writes nothing");
    }

    #[test]
    fn an_edited_skill_is_left_alone_and_a_proposal_written_beside_it() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = plant_an_edited_skill(home.path());

        let done = install_into(home.path(), false).expect("installing");

        assert_eq!(
            done,
            Plan::Propose {
                installed: path.clone(),
                proposed: proposed_path(&path),
            }
        );
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
        let path = plant_an_edited_skill(home.path());

        let done = install_into(home.path(), true).expect("installing");

        assert_eq!(done, Plan::Write(path.clone()));
        assert_eq!(status(&path), Status::Current);
        assert!(
            !proposed_path(&path).exists(),
            "forcing writes the skill itself, not a proposal"
        );
    }

    fn plant_an_edited_skill(cwd: &Path) -> PathBuf {
        let path = path(Scope::Project, cwd).expect("a path");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("creating the directory");
        std::fs::write(&path, "mine, edited").expect("writing an edited skill");
        path
    }

    /// Every component below the root is checked the same way, so the file and
    /// a directory above it are one case rather than two.
    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_symbolic_link() {
        for link_at in [".claude/skills", ".claude/skills/oboro/SKILL.md"] {
            let home = tempfile::tempdir().expect("temporary directory");
            let elsewhere = home.path().join("elsewhere");
            std::fs::create_dir_all(&elsewhere).expect("creating the link target");
            let link = home.path().join(link_at);
            std::fs::create_dir_all(link.parent().expect("a parent"))
                .expect("creating the directory");
            std::os::unix::fs::symlink(&elsewhere, &link).expect("linking");

            let error = install_into(home.path(), true).expect_err("must refuse");

            let reported = format!("{error:#}");
            assert!(
                reported.contains("symbolic link") && reported.contains(link_at),
                "a link at {link_at} must be named: {reported}"
            );
            assert_eq!(
                std::fs::read_dir(&elsewhere)
                    .expect("reading the link target")
                    .count(),
                0,
                "nothing is written through the link at {link_at}"
            );
        }
    }
}
