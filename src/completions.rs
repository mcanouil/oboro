//! Shell completion scripts, and where each shell expects to find one.
//!
//! Generating the script is half the job. A command that prints one and stops
//! has answered "what" and left "where" to the user, and "where" is the part
//! they cannot guess: every shell has its own convention and several need a
//! step beyond writing the file. So [`script`] goes to standard output and
//! [`hint`] to standard error, which keeps `oboro completions zsh > _oboro`
//! writing a valid script with no comment header to strip while the
//! instructions still reach the terminal, and lets `2>/dev/null` suppress them
//! for anyone scripting the command.

use std::path::PathBuf;

use clap_complete::Shell;

/// Where the destination table is written out in full, for the hint to point
/// at rather than repeat.
const DOCS: &str = "https://m.canouil.dev/oboro/reference.html#completions";

/// The completion script for `shell`.
///
/// Generated under the name the command carries rather than the name it was
/// invoked by, so a script written from `target/debug/oboro`, or from a renamed
/// copy, still completes the installed name.
#[must_use]
pub fn script(shell: Shell, command: &mut clap::Command) -> String {
    let name = command.get_name().to_owned();
    let mut buffer = Vec::new();
    clap_complete::generate(shell, command, name, &mut buffer);
    // Every generator writes its own source, which is UTF-8, and the only thing
    // interpolated into it comes from a command definition built from `&str`,
    // so there is nothing here to be invalid. Read lossily all the same: a
    // script with one character replaced is a great deal closer to working than
    // the empty one that refusing would leave behind.
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Where to put the script for `shell`, and what has to happen afterwards.
///
/// [`Shell`] is `#[non_exhaustive]`, so the wildcard arm names the shell rather
/// than saying nothing when a dependency upgrade adds one.
#[must_use]
pub fn hint(shell: Shell, name: &str) -> String {
    match shell {
        Shell::Bash => format!(
            "Write it as {name} in ~/.local/share/bash-completion/completions, \
             or under $XDG_DATA_HOME when you set one.\n\
             Nothing further is needed.\n\
             See {DOCS}\n"
        ),
        // oh-my-zsh is named first because it puts $ZSH_CUSTOM/completions on
        // $fpath and runs compinit itself, so those users are finished once the
        // file is written; the generic advice would send them to edit ~/.zshrc
        // for no effect. The manual route follows for everyone else, which
        // serves both without a detection step.
        Shell::Zsh => format!(
            "Write it as _{name} in a directory on $fpath.\n\
             With oh-my-zsh, ~/.oh-my-zsh/custom/completions needs nothing further.\n\
             Otherwise, such as ~/.zfunc, have compinit run after it is added:\n  \
             fpath=(~/.zfunc $fpath)\n  \
             autoload -Uz compinit && compinit\n\
             See {DOCS}\n"
        ),
        Shell::Fish => format!(
            "Write it as {name}.fish in ~/.config/fish/completions, \
             or under $XDG_CONFIG_HOME when you set one.\n\
             Nothing further is needed.\n\
             See {DOCS}\n"
        ),
        Shell::Elvish => format!(
            "Write it as {name}.elv in ~/.config/elvish/lib, \
             or under $XDG_CONFIG_HOME when you set one.\n\
             Then add this to ~/.config/elvish/rc.elv:\n  \
             use {name}\n\
             See {DOCS}\n"
        ),
        // Not a file: PowerShell evaluates this from the profile, so there is
        // usually nothing on disk to keep.
        Shell::PowerShell => format!(
            "PowerShell reads this from your profile rather than from a file:\n  \
             {name} completions powershell | Out-String | Invoke-Expression\n\
             Put that line in $PROFILE to have it in every session.\n\
             See {DOCS}\n"
        ),
        other => format!(
            "Write it where {other} reads completion scripts from; \
             Oboro does not know that shell's convention.\n\
             See {DOCS}\n"
        ),
    }
}

/// A place a shell conventionally reads a completion script from.
pub struct Location {
    /// The shell that reads it.
    pub shell: Shell,
    /// The file itself, resolved against the home directory and whichever of
    /// `$XDG_DATA_HOME`, `$XDG_CONFIG_HOME` and `$ZSH_CUSTOM` apply.
    pub path: PathBuf,
    /// How this place is written wherever it is named to a user: in [`hint`],
    /// in the command reference, and in `docs/install.sh`.
    ///
    /// The default form, so an override such as `$ZSH_CUSTOM` changes `path`
    /// and leaves this alone. The installer checks these same places in shell
    /// rather than in Rust, so the list is stated twice and can drift; this is
    /// what a test compares the two copies on, the home directory and the
    /// overrides being exactly what it cannot compare.
    pub convention: &'static str,
}

/// Every place a completion script for `name` conventionally lives.
///
/// zsh contributes two, because its directory is not fixed: whichever of
/// `$ZSH_CUSTOM/completions` and the oh-my-zsh default applies, and `~/.zfunc`.
/// PowerShell contributes none, being read from the profile rather than saved.
///
/// Empty when there is no home directory to resolve against.
#[must_use]
pub fn conventional_paths(name: &str) -> Vec<Location> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let data = directory_from("XDG_DATA_HOME", &home, ".local/share");
    let config = directory_from("XDG_CONFIG_HOME", &home, ".config");
    let zsh_custom = match std::env::var_os("ZSH_CUSTOM") {
        Some(custom) if !custom.is_empty() => PathBuf::from(custom),
        _ => home.join(".oh-my-zsh/custom"),
    };

    vec![
        Location {
            shell: Shell::Bash,
            path: data.join("bash-completion/completions").join(name),
            convention: "bash-completion/completions",
        },
        Location {
            shell: Shell::Zsh,
            path: zsh_custom.join("completions").join(format!("_{name}")),
            convention: ".oh-my-zsh/custom",
        },
        Location {
            shell: Shell::Zsh,
            path: home.join(".zfunc").join(format!("_{name}")),
            convention: ".zfunc",
        },
        Location {
            shell: Shell::Fish,
            path: config.join("fish/completions").join(format!("{name}.fish")),
            convention: "fish/completions",
        },
        Location {
            shell: Shell::Elvish,
            path: config.join("elvish/lib").join(format!("{name}.elv")),
            convention: "elvish/lib",
        },
    ]
}

/// The directory `variable` names, or `fallback` under the home directory.
///
/// An empty variable is treated as unset, which is what the XDG specification
/// says and what a shell profile that exports one unconditionally produces.
fn directory_from(variable: &str, home: &std::path::Path, fallback: &str) -> PathBuf {
    match std::env::var_os(variable) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(fallback),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser, ValueEnum};

    #[derive(Parser)]
    #[command(name = "example")]
    struct Example {
        /// Something to complete
        #[arg(long)]
        thing: Option<String>,
    }

    fn command() -> clap::Command {
        Example::command()
    }

    /// Driven from the framework's own list rather than a hand-written one, so
    /// a shell added by a dependency upgrade is caught here rather than
    /// silently falling to the wildcard arm.
    #[test]
    fn every_shell_generates_a_script_and_a_hint() {
        for shell in Shell::value_variants() {
            let mut command = command();
            let script = script(*shell, &mut command);
            assert!(
                script.contains("example"),
                "the {shell} script must name the command"
            );
            let hint = hint(*shell, "example");
            assert!(!hint.is_empty(), "the {shell} hint must say something");
        }
    }

    /// The name the command carries, not the name it was invoked by: a script
    /// written from a build directory has to complete the installed name.
    #[test]
    fn the_script_uses_the_command_name() {
        let mut command = command().name("renamed");
        let script = script(Shell::Bash, &mut command);
        assert!(script.contains("renamed"), "the new name must be used");
    }

    /// The split between standard output and standard error is the whole
    /// design, and a later refactor folding the hint into the script would
    /// leave prose in a file the shell has to source.
    #[test]
    fn the_hint_never_reaches_the_script() {
        for shell in Shell::value_variants() {
            let mut command = command();
            let script = script(*shell, &mut command);
            assert!(
                !script.contains(DOCS),
                "the {shell} script must not carry the hint"
            );
        }
    }

    /// Each arm must name its own destination, so they cannot collapse into one
    /// generic sentence that is true of nothing.
    #[test]
    fn each_shell_names_its_own_destination() {
        for (shell, expected) in [
            (Shell::Bash, "~/.local/share/bash-completion/completions"),
            (Shell::Zsh, "$fpath"),
            (Shell::Fish, "~/.config/fish/completions"),
            (Shell::Elvish, "~/.config/elvish/lib"),
            (Shell::PowerShell, "$PROFILE"),
        ] {
            let hint = hint(shell, "example");
            assert!(
                hint.contains(expected),
                "the {shell} hint must name {expected}, and said: {hint}"
            );
        }
    }

    /// The lines meant to be pasted are asserted exactly rather than by the
    /// concepts around them. `hint.contains("compinit")` passes against a hint
    /// that says `autoload -Uz MUTANT`, because the sentence introducing the
    /// command contains the word too.
    #[test]
    fn the_zsh_hint_prints_commands_that_work() {
        let hint = hint(Shell::Zsh, "example");
        assert!(hint.contains("fpath=(~/.zfunc $fpath)"));
        assert!(hint.contains("autoload -Uz compinit && compinit"));
    }

    /// The elvish script is inert until it is used, so the hint that stops at
    /// the file has not finished the job.
    #[test]
    fn the_elvish_hint_prints_the_line_that_loads_the_script() {
        let hint = hint(Shell::Elvish, "example");
        assert!(hint.contains("use example"));
        assert!(hint.contains("rc.elv"));
    }

    /// PowerShell is usually evaluated rather than saved, so the hint has to
    /// print the pipeline rather than name a path.
    #[test]
    fn the_powershell_hint_prints_the_pipeline() {
        let hint = hint(Shell::PowerShell, "example");
        assert!(hint.contains("example completions powershell | Out-String | Invoke-Expression"));
    }

    #[test]
    fn the_conventional_paths_name_the_script_per_shell_convention() {
        let paths = conventional_paths("example");
        assert!(!paths.is_empty(), "a home directory must yield paths");

        let names: Vec<_> = paths
            .iter()
            .map(|location| location.path.file_name().and_then(|name| name.to_str()))
            .collect();
        assert!(
            names.contains(&Some("example")),
            "bash reads the bare name: {names:?}"
        );
        // The leading underscore, which is easy to leave off and leaves a file
        // zsh never reads.
        assert!(
            names.contains(&Some("_example")),
            "zsh reads a leading underscore: {names:?}"
        );
        assert!(names.contains(&Some("example.fish")));
        assert!(names.contains(&Some("example.elv")));
    }

    /// PowerShell is deliberately absent: there is no file to look for, so
    /// reporting on one would be reporting on nothing.
    #[test]
    fn the_conventional_paths_leave_powershell_out() {
        assert!(
            !conventional_paths("example")
                .iter()
                .any(|location| location.shell == Shell::PowerShell)
        );
    }

    /// zsh has no fixed directory, so one entry cannot cover it.
    #[test]
    fn the_conventional_paths_cover_both_zsh_directories() {
        let paths = conventional_paths("example");
        let zsh: Vec<_> = paths
            .iter()
            .filter(|location| location.shell == Shell::Zsh)
            .map(|location| location.convention)
            .collect();
        assert!(zsh.contains(&".oh-my-zsh/custom"), "{zsh:?}");
        assert!(zsh.contains(&".zfunc"), "{zsh:?}");
    }

    /// A place `doctor` looks in that the hint never sends anyone to is a place
    /// nobody put a script, so the two lists have to name the same directories.
    #[test]
    fn the_hint_names_every_place_that_is_looked_in() {
        for location in conventional_paths("example") {
            let hint = hint(location.shell, "example");
            assert!(
                hint.contains(location.convention),
                "the {} hint must name {}, and said: {hint}",
                location.shell,
                location.convention
            );
        }
    }
}
