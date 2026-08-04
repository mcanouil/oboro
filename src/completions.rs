//! Shell completion scripts: printing one, and putting it where the shell
//! reads it.
//!
//! Generating the script is half the job. A command that prints one and stops
//! has answered "what" and left "where" to the user, and "where" is the part
//! they cannot guess: every shell has its own convention, several need a step
//! beyond writing the file, and zsh has no per-user directory on its default
//! `fpath` at all. So `oboro completions <shell>` still prints the script to
//! standard output and the destination to standard error, and
//! `oboro completions <shell> --install` carries it out.
//!
//! Nothing here reads the process environment on its own. [`Environment`] is
//! resolved once by the caller and threaded through, which keeps every decision
//! testable without mutating environment variables the rest of the test binary
//! is reading in parallel.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap_complete::Shell;

/// Where the per-shell instructions live, for the hint to point at when the few
/// lines it can spare are not enough.
pub const DOCS: &str = "https://m.canouil.dev/oboro/shells.html";

/// What opens the block this installer manages in a shell's configuration.
///
/// Written under the name the command carries, as the destinations are, so two
/// binaries never manage one another's block.
fn block_start(name: &str) -> String {
    format!("# >>> {name} completions >>>")
}

/// What closes it.
fn block_end(name: &str) -> String {
    format!("# <<< {name} completions <<<")
}

/// A completion function's file name, which zsh reads only with the leading
/// underscore it is easy to leave off.
fn underscored(name: &str) -> String {
    format!("_{name}")
}

/// The directories a destination is resolved against.
///
/// Held rather than read at each use, so the whole resolution can be tested
/// against a temporary directory and a run cannot answer differently in two
/// places because a variable changed underneath it.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The name the command carries, which every destination is named after.
    pub name: String,

    /// The home directory.
    pub home: PathBuf,

    /// `$XDG_DATA_HOME`, or `~/.local/share`.
    pub data: PathBuf,

    /// `$XDG_CONFIG_HOME`, or `~/.config`.
    pub config: PathBuf,

    /// Oh My Zsh's custom directory, whether or not it exists.
    pub oh_my_zsh: PathBuf,

    /// A Homebrew prefix holding a `share/zsh/site-functions` directory, when
    /// this machine has one.
    pub homebrew: Option<PathBuf>,
}

impl Environment {
    /// Resolves the destinations from the environment this process was given.
    ///
    /// # Errors
    ///
    /// Returns an error when there is no home directory, since every
    /// destination is named relative to it.
    pub fn from_env(name: &str) -> Result<Self> {
        let home = dirs::home_dir().context("finding the home directory")?;

        Ok(Self {
            name: name.to_owned(),
            data: directory_from("XDG_DATA_HOME", &home, ".local/share"),
            config: directory_from("XDG_CONFIG_HOME", &home, ".config"),
            oh_my_zsh: oh_my_zsh_from_env(&home),
            homebrew: homebrew_from_env(),
            home,
        })
    }

    /// Homebrew's completions directory, when there is one.
    fn brew_site_functions(&self) -> Option<PathBuf> {
        self.homebrew
            .as_ref()
            .map(|prefix| prefix.join("share/zsh/site-functions"))
    }
}

/// The directory `variable` names, or `fallback` under the home directory.
///
/// An empty variable is treated as unset, which is what the XDG specification
/// says and what a shell profile that exports one unconditionally produces.
fn directory_from(variable: &str, home: &Path, fallback: &str) -> PathBuf {
    match std::env::var_os(variable) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(fallback),
    }
}

/// Oh My Zsh's custom directory, honouring a relocated installation.
///
/// `$ZSH` and `$ZSH_CUSTOM` are exported by Oh My Zsh's stock configuration, so
/// they survive into any process it starts. They are followed only when they
/// point inside the home directory: a run with a different `$HOME` would
/// otherwise write into, and on uninstall delete from, a home nobody named.
fn oh_my_zsh_from_env(home: &Path) -> PathBuf {
    std::env::var_os("ZSH_CUSTOM")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ZSH").map(|zsh| PathBuf::from(zsh).join("custom")))
        .filter(|candidate| candidate.starts_with(home))
        .unwrap_or_else(|| home.join(".oh-my-zsh/custom"))
}

/// A Homebrew prefix whose completions directory exists.
///
/// `brew --prefix` is not run to find it: the answer is a directory test, and
/// starting Homebrew to ask costs about a second. `$HOMEBREW_PREFIX` is
/// authoritative when set, since a machine with a relocated prefix exports it.
fn homebrew_from_env() -> Option<PathBuf> {
    let has_completions = |prefix: &Path| prefix.join("share/zsh/site-functions").is_dir();

    if let Some(prefix) = std::env::var_os("HOMEBREW_PREFIX")
        .map(PathBuf::from)
        .filter(|prefix| !prefix.as_os_str().is_empty())
    {
        return has_completions(&prefix).then_some(prefix);
    }

    [PathBuf::from("/opt/homebrew"), PathBuf::from("/usr/local")]
        .into_iter()
        .find(|prefix| has_completions(prefix))
}

/// An edit to a shell's configuration that a destination needs to be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcEdit {
    /// The configuration file to write the managed block into.
    pub file: PathBuf,

    /// What goes inside the block.
    pub body: String,
}

/// A place a shell conventionally reads a completion script from.
#[derive(Debug, Clone)]
pub struct Location {
    /// The shell that reads it.
    pub shell: Shell,

    /// The file itself.
    pub path: PathBuf,

    /// How this place is written wherever it is named to a user: in [`hint`],
    /// in the documentation, and in `docs/install.sh`. A test compares this
    /// against that installer, the home directory and the environment
    /// overrides being exactly what it cannot compare.
    pub convention: &'static str,

    /// What the shell needs beyond the file, when it needs anything.
    pub rc: Option<RcEdit>,

    /// Whether this place is only ever cleaned up, never written to.
    ///
    /// Somewhere an older version, or an older set of instructions, put the
    /// script. A file found there is swept so it cannot shadow the one being
    /// maintained, and it is never chosen: choosing it would keep an install
    /// on the very layout this is meant to move it off.
    pub sweep_only: bool,
}

/// Every place a completion script for `shell` is read from, in the order an
/// install prefers them.
///
/// Two jobs. An install takes the first that already holds a file, so a
/// location chosen deliberately is not moved by installing Oh My Zsh or
/// Homebrew afterwards; and every other one is swept, so no copy is left for
/// nothing to keep up to date.
///
/// PowerShell contributes none, being evaluated from `$PROFILE` rather than
/// saved.
#[must_use]
pub fn known_locations(environment: &Environment, shell: Shell) -> Vec<Location> {
    // `compinit -i`, not `-C`. `-C` omits the check for new completion
    // functions and reuses the dump when one exists, and one always does here:
    // this block is appended below whatever compinit the user already calls, so
    // `_oboro` would never be picked up. `-i` keeps the check and only skips
    // the warning about insecure directories, which their own call has already
    // reported if there was anything to report.
    let zshrc = |directory: &Path| {
        Some(RcEdit {
            file: environment.home.join(".zshrc"),
            body: format!(
                "fpath=(\"{}\" $fpath)\nautoload -Uz compinit && compinit -i",
                directory.display()
            ),
        })
    };

    match shell {
        Shell::Bash => {
            let sourced = environment
                .data
                .join(format!("{name}-completions", name = environment.name))
                .join(format!("{name}.bash", name = environment.name));

            vec![
                Location {
                    shell,
                    // bash-completion loads this directory lazily, so the file
                    // on its own is enough.
                    path: environment
                        .data
                        .join("bash-completion/completions")
                        .join(&environment.name),
                    convention: "bash-completion/completions",
                    rc: None,
                    sweep_only: false,
                },
                Location {
                    shell,
                    rc: Some(RcEdit {
                        file: environment.home.join(".bashrc"),
                        body: format!(
                            "[ -r \"{path}\" ] && . \"{path}\"",
                            path = sourced.display()
                        ),
                    }),
                    path: sourced,
                    convention: "-completions",
                    sweep_only: false,
                },
            ]
        }
        Shell::Zsh => {
            let zfunc = environment.home.join(".zfunc");
            // Where a hand install following older instructions would have put
            // it. Listed so such a file is found and swept rather than left to
            // shadow the one being maintained.
            let site_functions = environment.data.join("zsh/site-functions");

            let mut locations = vec![Location {
                shell,
                // Oh My Zsh puts this on $fpath and runs compinit itself, both
                // before anything appended to ~/.zshrc could run, so the file
                // is all it needs.
                path: environment
                    .oh_my_zsh
                    .join("completions")
                    .join(underscored(&environment.name)),
                convention: ".oh-my-zsh/custom",
                rc: None,
                sweep_only: false,
            }];

            if let Some(directory) = environment.brew_site_functions() {
                // Homebrew's own shell setup puts this on $fpath for the same
                // reason.
                locations.push(Location {
                    shell,
                    path: directory.join(underscored(&environment.name)),
                    convention: "share/zsh/site-functions",
                    rc: None,
                    sweep_only: false,
                });
            }

            locations.push(Location {
                shell,
                path: zfunc.join(underscored(&environment.name)),
                convention: ".zfunc",
                rc: zshrc(&zfunc),
                sweep_only: false,
            });
            locations.push(Location {
                shell,
                path: site_functions.join(underscored(&environment.name)),
                convention: "zsh/site-functions",
                rc: zshrc(&site_functions),
                sweep_only: true,
            });

            locations
        }
        // fish autoloads this directory, so there is nothing to configure.
        Shell::Fish => vec![Location {
            shell,
            path: environment
                .config
                .join("fish/completions")
                .join(format!("{name}.fish", name = environment.name)),
            convention: "fish/completions",
            rc: None,
            sweep_only: false,
        }],
        // The elvish script is inert until it is used, so the file alone has
        // not finished the job.
        Shell::Elvish => vec![Location {
            shell,
            path: environment
                .config
                .join("elvish/lib")
                .join(format!("{name}.elv", name = environment.name)),
            convention: "elvish/lib",
            rc: Some(RcEdit {
                file: environment.config.join("elvish/rc.elv"),
                body: format!("use {}", environment.name),
            }),
            sweep_only: false,
        }],
        // PowerShell evaluates its script from $PROFILE, and a shell added by a
        // future clap_complete has no convention here to guess at.
        _ => Vec::new(),
    }
}

/// Every place any shell reads a completion script from.
///
/// What `doctor` walks, and what a test compares `docs/install.sh` against.
#[must_use]
pub fn conventional_paths(environment: &Environment) -> Vec<Location> {
    [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Elvish]
        .into_iter()
        .flat_map(|shell| known_locations(environment, shell))
        .collect()
}

/// Where a first install goes, given what this machine has installed.
fn default_location(environment: &Environment, shell: Shell) -> Option<Location> {
    known_locations(environment, shell)
        .into_iter()
        .filter(|location| !location.sweep_only)
        .find(|location| match location.convention {
            "bash-completion/completions" => environment
                .data
                .join("bash-completion/completions")
                .is_dir(),
            ".oh-my-zsh/custom" => environment.oh_my_zsh.is_dir(),
            // Writability is asked of Homebrew's directory and of nothing else:
            // it is the one candidate outside the user's home, and a prefix
            // installed by another user must not turn an install into a
            // permission error.
            "share/zsh/site-functions" => environment
                .brew_site_functions()
                .is_some_and(|directory| is_writable(&directory)),
            _ => true,
        })
}

/// Whether a directory can be written to.
///
/// Asked by attempting a write rather than by reading the mode, since the mode
/// accounts for neither the effective user, nor ACLs, nor a read-only mount.
fn is_writable(directory: &Path) -> bool {
    let probe = directory.join(".oboro-write-test");

    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// What an install would do.
#[derive(Debug)]
pub struct Plan {
    /// Where the script goes, and what that place needs.
    pub location: Location,

    /// Whether that file is already there, which is why it was chosen.
    pub existing: bool,

    /// Scripts in other known places, which this install removes.
    pub stale: Vec<PathBuf>,

    /// A configuration file holding a managed block this install does not want.
    pub stale_rc: Option<PathBuf>,
}

/// What installing for `shell` would do.
///
/// # Errors
///
/// Returns an error for a shell that reads its completions from somewhere other
/// than a file, naming what to do instead.
pub fn plan(environment: &Environment, shell: Shell) -> Result<Plan> {
    let locations = known_locations(environment, shell);
    if locations.is_empty() {
        bail!(
            "{shell} reads completions from its profile rather than from a file, \
             so there is nothing to install.\n{}",
            hint(shell, &environment.name)
        );
    }

    // Sweep-only places are skipped here as well as in `default_location`:
    // keeping an install on one of them would leave it exactly where this is
    // meant to move it off.
    let existing = locations
        .iter()
        .find(|location| !location.sweep_only && location.path.is_file());
    let chosen = match existing {
        Some(location) => location.clone(),
        None => default_location(environment, shell)
            .context("no conventional destination for this shell")?,
    };

    let stale = locations
        .iter()
        .filter(|location| location.path != chosen.path && location.path.is_file())
        .map(|location| location.path.clone())
        .collect();

    Ok(Plan {
        stale_rc: stale_rc(&locations, chosen.rc.as_ref(), &environment.name),
        existing: existing.is_some(),
        location: chosen,
        stale,
    })
}

/// A configuration file carrying a managed block the chosen destination does
/// not want.
fn stale_rc(locations: &[Location], keeping: Option<&RcEdit>, name: &str) -> Option<PathBuf> {
    locations
        .iter()
        .filter_map(|location| location.rc.as_ref())
        .map(|rc| rc.file.clone())
        .find(|file| {
            keeping.is_none_or(|keeping| &keeping.file != file) && block_present(file, name)
        })
}

/// Carries `plan` out, returning what happened.
///
/// # Errors
///
/// Returns an error when a file cannot be written or removed.
pub fn install(environment: &Environment, plan: &Plan, script: &str) -> Result<String> {
    let mut report = String::new();

    if let Some(parent) = plan.location.path.parent() {
        create_directory(parent)?;
    }

    // A script that already matches is the script that would be written, so
    // saying so and leaving it alone keeps a re-run over an unchanged build
    // quiet.
    if std::fs::read_to_string(&plan.location.path).is_ok_and(|found| found == script) {
        let _ = writeln!(report, "Already current {}", plan.location.path.display());
    } else {
        write_file(&plan.location.path, script)?;
        let verb = if plan.existing {
            "Updated"
        } else {
            "Installed"
        };
        let _ = writeln!(report, "{verb} {}", plan.location.path.display());
    }

    if let Some(rc) = &plan.location.rc {
        write_block(&rc.file, &rc.body, &environment.name)?;
        let _ = writeln!(report, "Updated {}", rc.file.display());
    }

    for path in &plan.stale {
        remove_file(path)?;
        let _ = writeln!(report, "Removed {}", path.display());
    }

    if let Some(file) = &plan.stale_rc {
        remove_block(file, &environment.name)?;
        let _ = writeln!(report, "Cleaned {}", file.display());
    }

    let _ = write!(report, "\nStart a new shell to use them.\nSee {DOCS}");

    // No trailing newline: every report here ends where it ends, and the caller
    // adds the line break its own output convention wants.
    Ok(report.trim_end().to_owned())
}

/// Says what [`install`] would do, and does nothing.
#[must_use]
pub fn describe(plan: &Plan) -> String {
    let mut report = String::new();

    let verb = if plan.existing {
        "Would update"
    } else {
        "Would write "
    };
    let _ = writeln!(report, "{verb} {}", plan.location.path.display());

    if let Some(rc) = &plan.location.rc {
        let _ = writeln!(report, "Would update {} (managed block)", rc.file.display());
    }
    for path in &plan.stale {
        let _ = writeln!(report, "Would remove {}", path.display());
    }
    if let Some(file) = &plan.stale_rc {
        let _ = writeln!(report, "Would clean  {} (managed block)", file.display());
    }

    let _ = write!(report, "\n--dry-run: nothing was written.");

    report.trim_end().to_owned()
}

/// Removes every script and managed block for `shell`, returning what happened.
///
/// # Errors
///
/// Returns an error when a file cannot be removed or rewritten.
pub fn uninstall(environment: &Environment, shell: Shell, dry_run: bool) -> Result<String> {
    let locations = known_locations(environment, shell);

    let scripts: Vec<_> = locations
        .iter()
        .map(|location| location.path.clone())
        .filter(|path| path.is_file())
        .collect();
    // Deduplicated: several destinations for one shell name the same
    // configuration file, and there is only ever one block in it to remove.
    // Kept in the order the destinations are listed rather than sorted, so the
    // report reads in the order an install would have written them.
    let mut blocks: Vec<PathBuf> = Vec::new();
    for file in locations
        .iter()
        .filter_map(|location| location.rc.as_ref().map(|rc| rc.file.clone()))
    {
        if !blocks.contains(&file) && block_present(&file, &environment.name) {
            blocks.push(file);
        }
    }

    if scripts.is_empty() && blocks.is_empty() {
        return Ok(format!("Nothing to remove for {shell}."));
    }

    let mut report = String::new();

    for path in &scripts {
        if dry_run {
            let _ = writeln!(report, "Would remove {}", path.display());
        } else {
            remove_file(path)?;
            let _ = writeln!(report, "Removed {}", path.display());
        }
    }
    for file in &blocks {
        if dry_run {
            let _ = writeln!(report, "Would clean  {} (managed block)", file.display());
        } else {
            remove_block(file, &environment.name)?;
            let _ = writeln!(report, "Cleaned {}", file.display());
        }
    }

    if dry_run {
        let _ = write!(report, "\n--dry-run: nothing was removed.");
    }

    Ok(report.trim_end().to_owned())
}

/// The shell `$SHELL` names, for a run that named none.
///
/// # Errors
///
/// Returns an error when `$SHELL` is unset or names a shell with no convention
/// here, since guessing would install a script the shell in use never reads.
pub fn shell_from_env() -> Result<Shell> {
    let found = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty());

    let name = found
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .and_then(|name| name.to_str());

    match name {
        Some("bash") => Ok(Shell::Bash),
        Some("zsh") => Ok(Shell::Zsh),
        Some("fish") => Ok(Shell::Fish),
        Some("elvish") => Ok(Shell::Elvish),
        Some("pwsh" | "powershell") => Ok(Shell::PowerShell),
        _ => bail!(
            "cannot tell which shell to use from $SHELL{}; \
             name one, as in `oboro completions zsh`",
            found.map_or_else(String::new, |found| format!(" ({found})"))
        ),
    }
}

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

/// What to do with the script a shell was just handed.
///
/// Printed to standard error rather than returned with the script, so that
/// `oboro completions zsh > _oboro` writes the script alone while the
/// instructions still reach the terminal.
#[must_use]
pub fn hint(shell: Shell, name: &str) -> String {
    let steps = match shell {
        Shell::Bash => [
            format!("Install it with `{name} completions bash --install`, which writes it to"),
            "~/.local/share/bash-completion/completions when that directory exists,".to_owned(),
            format!("and to ~/.local/share/{name}-completions with a line in ~/.bashrc otherwise."),
        ]
        .join("\n"),
        // Named in the order an install prefers them, so the first line that
        // applies is the one to read.
        Shell::Zsh => [
            format!("Install it with `{name} completions zsh --install`, which writes it as"),
            format!("_{name} in a directory on $fpath:"),
            "  ~/.oh-my-zsh/custom/completions under oh-my-zsh, which needs nothing further,"
                .to_owned(),
            "  $HOMEBREW_PREFIX/share/zsh/site-functions under Homebrew, likewise,".to_owned(),
            "  ~/.zfunc otherwise, with this added to ~/.zshrc:".to_owned(),
            "    fpath=(~/.zfunc $fpath)".to_owned(),
            "    autoload -Uz compinit && compinit -i".to_owned(),
        ]
        .join("\n"),
        Shell::Fish => [
            format!("Install it with `{name} completions fish --install`, or write it to"),
            format!("~/.config/fish/completions/{name}.fish yourself."),
        ]
        .join("\n"),
        Shell::Elvish => [
            format!("Install it with `{name} completions elvish --install`, which writes it to"),
            format!("~/.config/elvish/lib/{name}.elv and loads it from rc.elv:"),
            format!("  use {name}"),
        ]
        .join("\n"),
        Shell::PowerShell => [
            "Evaluate it rather than saving it:".to_owned(),
            format!("  {name} completions powershell | Out-String | Invoke-Expression"),
            "Add that line to the file $PROFILE names to have it apply to every session."
                .to_owned(),
        ]
        .join("\n"),
        // `Shell` is non-exhaustive, so a shell added by a future
        // `clap_complete` still gets pointed somewhere useful rather than
        // failing to compile or saying nothing at all.
        _ => format!("Install it where {shell} looks for completion scripts for {name}."),
    };

    format!("{steps}\nSee {DOCS}")
}

/// Whether `file` carries the block this installer manages.
fn block_present(file: &Path, name: &str) -> bool {
    let start = block_start(name);

    std::fs::read_to_string(file)
        .is_ok_and(|contents| contents.lines().any(|line| line.trim_end() == start))
}

/// Writes the managed block into `file`, replacing any block already there.
fn write_block(file: &Path, body: &str, name: &str) -> Result<()> {
    let existing = std::fs::read_to_string(file).unwrap_or_default();
    let mut kept = without_block(&existing, name);

    if !kept.is_empty() && !kept.ends_with('\n') {
        kept.push('\n');
    }

    if let Some(parent) = file.parent() {
        create_directory(parent)?;
    }

    write_file(
        file,
        &format!(
            "{kept}{start}\n{body}\n{end}\n",
            start = block_start(name),
            end = block_end(name)
        ),
    )
}

/// Removes the managed block from `file`, leaving everything else alone.
fn remove_block(file: &Path, name: &str) -> Result<()> {
    let Ok(existing) = std::fs::read_to_string(file) else {
        return Ok(());
    };

    write_file(file, &without_block(&existing, name))
}

/// `contents` without the managed block.
fn without_block(contents: &str, name: &str) -> String {
    let start = block_start(name);
    let end = block_end(name);
    let mut kept = String::new();
    let mut inside = false;

    for line in contents.lines() {
        let line = line.trim_end();

        if line == start {
            inside = true;
        } else if line == end {
            inside = false;
        } else if !inside {
            kept.push_str(line);
            kept.push('\n');
        }
    }

    kept
}

fn create_directory(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

fn remove_file(path: &Path) -> Result<()> {
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    /// An environment rooted in a temporary directory, with no layout present
    /// until a test creates one.
    fn environment(home: &Path) -> Environment {
        Environment {
            name: "oboro".to_owned(),
            home: home.to_path_buf(),
            data: home.join(".local/share"),
            config: home.join(".config"),
            oh_my_zsh: home.join(".oh-my-zsh/custom"),
            homebrew: None,
        }
    }

    fn plant(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("a parent directory"))
            .expect("creating the directory");
        std::fs::write(path, "planted").expect("planting a file");
    }

    /// A command with the same name the binary carries, so the generated
    /// script is the one a user would install.
    fn command() -> clap::Command {
        clap::Command::new("oboro").subcommand(clap::Command::new("clean"))
    }

    #[test]
    fn every_shell_generates_a_script_and_a_hint() {
        for shell in Shell::value_variants() {
            let script = script(*shell, &mut command());
            assert!(
                script.contains("oboro"),
                "the {shell} script must name the command"
            );
            assert!(
                !hint(*shell, "oboro").is_empty(),
                "the {shell} hint must say something"
            );
        }
    }

    /// The name the command carries, not the name it was invoked by: a script
    /// written from a build directory has to complete the installed name.
    #[test]
    fn the_script_uses_the_command_name() {
        let script = script(Shell::Bash, &mut command().name("renamed"));

        assert!(script.contains("renamed"), "the new name must be used");
    }

    /// The split between the script and the instructions is the whole design,
    /// and a refactor folding one into the other would leave prose in a file
    /// the shell has to source.
    #[test]
    fn the_hint_never_reaches_the_script() {
        for shell in Shell::value_variants() {
            assert!(
                !script(*shell, &mut command()).contains(DOCS),
                "the {shell} script must not carry the hint"
            );
        }
    }

    #[test]
    fn a_first_zsh_install_goes_to_zfunc() {
        let home = tempfile::tempdir().expect("a temporary home");

        let plan = plan(&environment(home.path()), Shell::Zsh).expect("a plan");

        assert_eq!(plan.location.path, home.path().join(".zfunc/_oboro"));
        assert!(!plan.existing);
        assert!(plan.location.rc.is_some(), "~/.zfunc needs an fpath line");
    }

    #[test]
    fn oh_my_zsh_wins_when_it_is_installed_and_needs_no_configuration() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        std::fs::create_dir_all(&environment.oh_my_zsh).expect("an oh-my-zsh layout");

        let plan = plan(&environment, Shell::Zsh).expect("a plan");

        assert_eq!(
            plan.location.path,
            environment.oh_my_zsh.join("completions/_oboro")
        );
        assert!(plan.location.rc.is_none(), "oh-my-zsh needs no rc edit");
    }

    #[test]
    fn homebrew_wins_over_zfunc_and_needs_no_configuration() {
        let home = tempfile::tempdir().expect("a temporary home");
        let prefix = home.path().join("brew");
        std::fs::create_dir_all(prefix.join("share/zsh/site-functions")).expect("a brew layout");
        let environment = Environment {
            homebrew: Some(prefix.clone()),
            ..environment(home.path())
        };

        let plan = plan(&environment, Shell::Zsh).expect("a plan");

        assert_eq!(
            plan.location.path,
            prefix.join("share/zsh/site-functions/_oboro")
        );
        assert!(plan.location.rc.is_none(), "Homebrew needs no rc edit");
    }

    /// The layout deciding where a *first* install goes must not move one that
    /// is already there: it may be where it is on purpose, and the line that
    /// reaches it is already in the user's configuration.
    #[test]
    fn an_existing_install_is_kept_where_it_is() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        plant(&home.path().join(".zfunc/_oboro"));
        std::fs::create_dir_all(&environment.oh_my_zsh).expect("an oh-my-zsh layout");

        let plan = plan(&environment, Shell::Zsh).expect("a plan");

        assert_eq!(plan.location.path, home.path().join(".zfunc/_oboro"));
        assert!(plan.existing);
        assert!(plan.stale.is_empty());
    }

    /// Two copies is what an install followed by a hand install leaves. The
    /// higher-precedence one is kept and the other goes, or the one left
    /// behind shadows the one being maintained.
    #[test]
    fn the_copies_that_are_not_kept_are_swept() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        let omz = environment.oh_my_zsh.join("completions/_oboro");
        let xdg = home.path().join(".local/share/zsh/site-functions/_oboro");
        plant(&omz);
        plant(&xdg);

        let plan = plan(&environment, Shell::Zsh).expect("a plan");

        assert_eq!(plan.location.path, omz);
        assert_eq!(plan.stale, vec![xdg]);
    }

    #[test]
    fn installing_writes_the_script_and_the_block_and_sweeps_the_rest() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        let xdg = home.path().join(".local/share/zsh/site-functions/_oboro");
        plant(&xdg);

        let plan = plan(&environment, Shell::Zsh).expect("a plan");
        install(&environment, &plan, "#compdef oboro\n").expect("the install to succeed");

        let installed = home.path().join(".zfunc/_oboro");
        assert_eq!(
            std::fs::read_to_string(&installed).expect("the script"),
            "#compdef oboro\n"
        );
        assert!(!xdg.exists(), "the copy that is not kept must go");

        let zshrc = std::fs::read_to_string(home.path().join(".zshrc")).expect("the rc file");
        assert_eq!(zshrc.matches(&block_start("oboro")).count(), 1);
        assert!(zshrc.contains(".zfunc"), "{zshrc}");
    }

    /// Re-running is the ordinary case, and a second block would mean a second
    /// compinit on every shell start.
    #[test]
    fn re_running_replaces_the_block_rather_than_appending_one() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        std::fs::write(home.path().join(".zshrc"), "# mine\n").expect("an rc file");

        for _ in 0..3 {
            let plan = plan(&environment, Shell::Zsh).expect("a plan");
            install(&environment, &plan, "#compdef oboro\n").expect("the install to succeed");
        }

        let zshrc = std::fs::read_to_string(home.path().join(".zshrc")).expect("the rc file");
        assert_eq!(zshrc.matches(&block_start("oboro")).count(), 1, "{zshrc}");
        assert_eq!(zshrc.matches(&block_end("oboro")).count(), 1, "{zshrc}");
        assert!(
            zshrc.starts_with("# mine\n"),
            "the user's own lines stay: {zshrc}"
        );
    }

    #[test]
    fn an_unchanged_script_is_reported_rather_than_rewritten() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());

        let first = plan(&environment, Shell::Zsh).expect("a plan");
        install(&environment, &first, "#compdef oboro\n").expect("the first install");
        let second = plan(&environment, Shell::Zsh).expect("a plan");
        let report =
            install(&environment, &second, "#compdef oboro\n").expect("the second install");

        assert!(report.contains("Already current"), "{report}");
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let home = tempfile::tempdir().expect("a temporary home");

        let plan = plan(&environment(home.path()), Shell::Zsh).expect("a plan");
        let report = describe(&plan);

        assert!(report.contains("Would write"), "{report}");
        assert!(!home.path().join(".zfunc/_oboro").exists());
        assert!(!home.path().join(".zshrc").exists());
    }

    #[test]
    fn uninstalling_removes_every_copy_and_the_block() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        let plan = plan(&environment, Shell::Zsh).expect("a plan");
        install(&environment, &plan, "#compdef oboro\n").expect("the install to succeed");
        plant(&home.path().join(".local/share/zsh/site-functions/_oboro"));

        uninstall(&environment, Shell::Zsh, false).expect("the uninstall to succeed");

        assert!(!home.path().join(".zfunc/_oboro").exists());
        assert!(
            !home
                .path()
                .join(".local/share/zsh/site-functions/_oboro")
                .exists()
        );
        let zshrc = std::fs::read_to_string(home.path().join(".zshrc")).expect("the rc file");
        assert!(!zshrc.contains(&block_start("oboro")), "{zshrc}");
    }

    /// Several zsh destinations name `~/.zshrc`, and there is one block in it,
    /// so reporting it once per destination would claim work that never
    /// happened.
    #[test]
    fn a_configuration_file_shared_by_two_destinations_is_reported_once() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        let plan = plan(&environment, Shell::Zsh).expect("a plan");
        install(&environment, &plan, "#compdef oboro\n").expect("the install to succeed");

        let report = uninstall(&environment, Shell::Zsh, false).expect("a report");

        assert_eq!(report.matches("Cleaned").count(), 1, "{report}");
    }

    /// Saying nothing at all reads as a broken command, and this is the first
    /// thing someone runs when completions are not working.
    #[test]
    fn uninstalling_a_clean_home_still_reports() {
        let home = tempfile::tempdir().expect("a temporary home");

        let report = uninstall(&environment(home.path()), Shell::Zsh, false).expect("a report");

        assert!(report.contains("Nothing to remove"), "{report}");
    }

    #[test]
    fn a_dry_run_uninstall_removes_nothing() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());
        let plan = plan(&environment, Shell::Zsh).expect("a plan");
        install(&environment, &plan, "#compdef oboro\n").expect("the install to succeed");

        let report = uninstall(&environment, Shell::Zsh, true).expect("a report");

        assert!(report.contains("Would remove"), "{report}");
        assert!(home.path().join(".zfunc/_oboro").exists());
    }

    /// PowerShell has no file to write, so an install has to refuse and say
    /// what to do instead rather than inventing a destination.
    #[test]
    fn powershell_refuses_to_be_installed_and_says_what_to_do() {
        let home = tempfile::tempdir().expect("a temporary home");

        let error = plan(&environment(home.path()), Shell::PowerShell)
            .expect_err("PowerShell to be refused");

        let message = error.to_string();
        assert!(message.contains("Invoke-Expression"), "{message}");
    }

    #[test]
    fn bash_takes_the_bash_completion_directory_only_when_it_exists() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = environment(home.path());

        let sourced = plan(&environment, Shell::Bash).expect("a plan");
        assert_eq!(
            sourced.location.path,
            home.path()
                .join(".local/share/oboro-completions/oboro.bash")
        );
        assert!(sourced.location.rc.is_some(), "it has to be sourced");

        std::fs::create_dir_all(environment.data.join("bash-completion/completions"))
            .expect("a bash-completion layout");

        let lazy = plan(&environment, Shell::Bash).expect("a plan");
        assert_eq!(
            lazy.location.path,
            home.path()
                .join(".local/share/bash-completion/completions/oboro")
        );
        assert!(
            lazy.location.rc.is_none(),
            "that directory is loaded lazily"
        );
    }

    /// `$ZSH` and `$ZSH_CUSTOM` reach any process Oh My Zsh starts. Following
    /// one out of `$HOME` would install into, and on uninstall delete from, a
    /// home nobody named.
    #[test]
    fn an_oh_my_zsh_outside_the_home_is_not_followed() {
        let home = tempfile::tempdir().expect("a temporary home");
        let elsewhere = tempfile::tempdir().expect("another directory");

        let resolved = oh_my_zsh_from_env(home.path());

        assert!(
            resolved.starts_with(home.path()),
            "resolved to {}",
            resolved.display()
        );
        assert!(!resolved.starts_with(elsewhere.path()));
    }

    #[test]
    fn every_installable_shell_names_its_file_per_convention() {
        let home = tempfile::tempdir().expect("a temporary home");

        let names: Vec<_> = conventional_paths(&environment(home.path()))
            .iter()
            .filter_map(|location| {
                location
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToOwned::to_owned)
            })
            .collect();

        assert!(names.contains(&"oboro".to_owned()), "{names:?}");
        // The leading underscore, which is easy to leave off and leaves a file
        // zsh never reads.
        assert!(names.contains(&"_oboro".to_owned()), "{names:?}");
        assert!(names.contains(&"oboro.fish".to_owned()), "{names:?}");
        assert!(names.contains(&"oboro.elv".to_owned()), "{names:?}");
    }

    /// PowerShell is deliberately absent: there is no file to look for, so
    /// reporting on one would be reporting on nothing.
    #[test]
    fn powershell_contributes_no_location() {
        let home = tempfile::tempdir().expect("a temporary home");

        assert!(known_locations(&environment(home.path()), Shell::PowerShell).is_empty());
    }

    /// A place `doctor` looks in that the hint never sends anyone to is a place
    /// nobody put a script.
    #[test]
    fn the_hint_names_every_place_that_is_written_to() {
        let home = tempfile::tempdir().expect("a temporary home");
        let environment = Environment {
            homebrew: Some(PathBuf::from("/opt/homebrew")),
            ..environment(home.path())
        };

        for location in conventional_paths(&environment) {
            // The XDG directory is swept rather than offered, so the hint has
            // nothing to say about it.
            if location.convention == "zsh/site-functions" {
                continue;
            }

            let hint = hint(location.shell, "oboro");
            assert!(
                hint.contains(location.convention),
                "the {} hint must name {}, and said: {hint}",
                location.shell,
                location.convention
            );
        }
    }
}
