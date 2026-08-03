//! Command line entry point.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, CommandFactory, Parser, Subcommand};

use oboro::claude::{SCOPES, Scope};
use oboro::config::{self, Config, RegionSource};
use oboro::convert;
use oboro::detect::Detector;
use oboro::hooks::{Change, EnabledPlugin};
use oboro::pipeline;
use oboro::skill::{Plan, Status};
use oboro::vault::{self, Vault};

#[derive(Parser)]
#[command(
    name = "oboro",
    version,
    about = "Anonymise files before sharing them with a language model",
    long_about = "Replaces sensitive values with stable placeholders, keeping the mapping in a \
                  local encrypted vault so answers can be restored afterwards."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Anonymise files into sanitised copies
    Clean {
        /// Files or directories to anonymise, or `-` for standard input
        #[arg(value_name = "PATH")]
        files: Vec<PathBuf>,
        /// Descend into subdirectories of any directory argument
        #[arg(short, long)]
        recursive: bool,
        /// Directory for the sanitised output (defaults to alongside each input)
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// Write to standard output instead of a file (one input only)
        #[arg(long, conflicts_with = "output")]
        stdout: bool,
        /// Configuration file (defaults to the nearest oboro.toml)
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Put real values back into a model's answer
    Restore {
        /// File containing placeholders, or `-` for standard input
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
        /// Write to standard output instead of a file
        #[arg(long)]
        stdout: bool,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Inspect or wipe the placeholder mapping
    Map {
        #[command(subcommand)]
        action: MapAction,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Fetch or inspect the local recognition model
    #[cfg(feature = "ner")]
    Models {
        #[command(subcommand)]
        action: ModelAction,
    },
    /// Review detections before writing, accepting or rejecting each
    Review {
        /// Files or directories to review
        #[arg(required = true, value_name = "PATH")]
        files: Vec<PathBuf>,
        /// Descend into subdirectories of any directory argument
        #[arg(short, long)]
        recursive: bool,
        /// Directory for the sanitised output (defaults to alongside each input)
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// Configuration file (defaults to the nearest oboro.toml)
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Answer an agent's hook, cleaning what it is about to be shown
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Tell an agent what the hooks have done to what it reads
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Serve the Model Context Protocol on standard input and output
    ///
    /// Exposes two tools, `clean` and `map_list`, to an agent in a client with
    /// no hook system. Restoring is deliberately not offered: over this
    /// protocol the caller is the model, and a model that can write a file and
    /// read it back could use a restore tool to obtain every value the vault
    /// holds.
    Mcp {
        /// Configuration file (defaults to the nearest oboro.toml)
        ///
        /// Worth setting. A client launches this server with a working
        /// directory of its own choosing, so without it the nearest
        /// `oboro.toml` is unlikely to be the project's, and the server runs
        /// on the defaults with no denylist and no custom patterns.
        #[arg(long, value_name = "FILE", env = "OBORO_CONFIG")]
        config: Option<PathBuf>,
        /// Directory the `clean` tool may read within; repeat for more
        ///
        /// The caller over this protocol is a model, not you, and a client
        /// that offers "always allow" on a tool call turns one approval into a
        /// standing licence. Naming roots is therefore the default and reading
        /// anywhere has to be asked for with `--unconfined`.
        #[arg(long, value_name = "DIR", conflicts_with = "unconfined")]
        root: Vec<PathBuf>,
        /// Let the `clean` tool read any file you can read
        ///
        /// The behaviour of the command line, chosen deliberately rather than
        /// arrived at by leaving `--root` off.
        #[arg(long)]
        unconfined: bool,
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Report the tool's configuration and environment
    Doctor {
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Print a shell completion script
    ///
    /// The script goes to standard output and the destination it belongs in
    /// goes to standard error alongside it, so `oboro completions zsh >
    /// _oboro` writes a file the shell can read and still tells you where to
    /// put it. Redirect standard error away to suppress that.
    ///
    /// The destinations are listed at
    /// https://m.canouil.dev/oboro/reference.html#completions
    // This doc comment is help text before it is documentation, and the fix
    // `doc_markdown` asks for, wrapping the URL in angle brackets, would print
    // them to anyone running `oboro completions --help`, where they read as a
    // placeholder to substitute rather than as a link to follow.
    #[expect(clippy::doc_markdown, reason = "a bare URL is what the help shows")]
    Completions {
        /// The shell to generate for
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Name both hooks in your agent's settings
    ///
    /// Without `--project` or `--user` you are asked which one to write to.
    /// Nothing already in the file is moved, reordered or removed, and a hook
    /// Oboro finds already named is left exactly as you wrote it.
    Install {
        /// Write `.claude/settings.local.json` here, covering this project
        #[arg(long, conflicts_with = "user")]
        project: bool,
        /// Write `~/.claude/settings.json`, covering every project
        #[arg(long)]
        user: bool,
        /// Print the settings that would be written and stop
        #[arg(long)]
        dry_run: bool,
    },
    /// Clean a tool's result before the model reads it
    ///
    /// Reads a Claude Code `PostToolUse` payload on standard input and writes
    /// the reply that replaces the tool's result.
    PostToolUse {
        #[command(flatten)]
        store: StoreArgs,
    },
    /// Put real values back into a tool's arguments before it runs
    ///
    /// Reads a Claude Code `PreToolUse` payload on standard input and writes
    /// the reply that replaces the tool's arguments, so a placeholder the model
    /// echoed back never reaches a file.
    PreToolUse {
        #[command(flatten)]
        store: StoreArgs,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// Write the skill into a `.claude/skills` directory
    ///
    /// Without `--project` or `--user` you are asked which one to write to,
    /// since installing into the wrong scope is a silent no-op rather than an
    /// error: the agent simply never reads it.
    ///
    /// `--with-hooks` installs both halves at once, into the same scope, and is
    /// the same merge `oboro hook install` does.
    Install {
        /// Install into `.claude/skills` here, covering this project
        #[arg(long, conflicts_with = "user")]
        project: bool,
        /// Install into `~/.claude/skills`, covering every project
        #[arg(long)]
        user: bool,
        /// Print the path that would be written and stop
        #[arg(long)]
        dry_run: bool,
        /// Overwrite an edited skill instead of proposing the new text beside it
        #[arg(long)]
        force: bool,
        /// Name both hooks in your agent's settings as well, in the same scope
        #[arg(long)]
        with_hooks: bool,
    },
    /// Print the skill this build carries
    Show,
}

#[cfg(feature = "ner")]
#[derive(Subcommand)]
enum ModelAction {
    /// Download the model, verifying it against pinned hashes
    Pull,
    /// Report what is installed
    Status,
}

#[derive(Subcommand)]
enum MapAction {
    /// List stored placeholders
    List {
        /// Also print the real values they stand for
        #[arg(long)]
        reveal: bool,
    },
    /// Delete every mapping, making existing sanitised output unrecoverable
    Purge {
        /// Confirm the deletion
        #[arg(long)]
        yes: bool,
    },
}

/// Where the vault and its key live, for the commands that open one.
///
/// Flattened onto those commands rather than onto the root, so a command that
/// never touches the vault does not list two flags it would ignore, and the
/// completion script does not offer them there either.
///
/// `global` is kept, because it makes a group cover its own subcommands: it is
/// declared once on `map` and reaches `map list` and `map purge`, so both
/// `oboro map --vault X list` and `oboro map list --vault X` parse without
/// `StoreArgs` being repeated on every leaf.
#[derive(Args, Clone)]
struct StoreArgs {
    /// Vault database (defaults to ~/.oboro/vault.db)
    ///
    /// The environment variable exists so a container can point the vault at
    /// a mounted volume without every command having to repeat the flag.
    #[arg(long, value_name = "FILE", global = true, env = "OBORO_VAULT")]
    vault: Option<PathBuf>,
    /// Encryption key file (defaults to ~/.oboro/key)
    #[arg(long, value_name = "FILE", global = true, env = "OBORO_KEY_FILE")]
    key: Option<PathBuf>,
}

impl StoreArgs {
    /// The vault and key paths, falling back to the defaults under `~/.oboro`.
    fn paths(&self) -> Result<(PathBuf, PathBuf)> {
        let db = match &self.vault {
            Some(path) => path.clone(),
            None => vault::default_db_path()?,
        };
        let key = match &self.key {
            Some(path) => path.clone(),
            None => vault::default_key_path()?,
        };
        Ok((db, key))
    }

    fn open(&self) -> Result<Vault> {
        let (db, key) = self.paths()?;
        Vault::open(&db, &key)
    }
}

fn main() {
    if let Err(error) = run() {
        oboro::note!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Clean {
            files,
            recursive,
            output,
            stdout,
            config,
            store,
        } => clean(
            &files,
            recursive,
            output.as_deref(),
            stdout,
            &store,
            config.as_deref(),
        ),
        Command::Restore {
            file,
            stdout,
            store,
        } => restore(file.as_deref(), stdout, &store),
        Command::Map { action, store } => match action {
            MapAction::List { reveal } => map_list(reveal, &store),
            MapAction::Purge { yes } => map_purge(yes, &store),
        },
        #[cfg(feature = "ner")]
        Command::Models { action } => match action {
            ModelAction::Pull => oboro::models::pull(),
            ModelAction::Status => print_stdout(&oboro::models::status()?),
        },
        Command::Review {
            files,
            recursive,
            output,
            config,
            store,
        } => review(
            &files,
            recursive,
            output.as_deref(),
            &store,
            config.as_deref(),
        ),
        Command::Hook { action } => match action {
            HookAction::Install {
                project,
                user,
                dry_run,
            } => hook_install(chosen_scope(project, user), dry_run),
            HookAction::PostToolUse { store } => hook_post_tool_use(&store),
            HookAction::PreToolUse { store } => hook_pre_tool_use(&store),
        },
        Command::Skill { action } => match action {
            SkillAction::Install {
                project,
                user,
                dry_run,
                force,
                with_hooks,
            } => skill_install(chosen_scope(project, user), dry_run, force, with_hooks),
            SkillAction::Show => print_stdout(oboro::skill::SKILL),
        },
        Command::Mcp {
            config,
            root,
            unconfined,
            store,
        } => mcp(config.as_deref(), &root, unconfined, &store),
        Command::Doctor { store } => doctor(&store),
        Command::Completions { shell } => completions(shell),
    }
}

/// Prints the completion script for `shell`, and where it belongs.
///
/// The script alone answers "what" and leaves "where" to the user, which is the
/// part they cannot guess. Sending the two to different streams is what lets
/// one command do both: the redirect captures the script, the hint stays on the
/// terminal where it is useful at that moment, and `2>/dev/null` drops it for
/// anyone scripting this.
fn completions(shell: clap_complete::Shell) -> Result<()> {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    print_stdout(&oboro::completions::script(shell, &mut command))?;
    oboro::note!("{}", oboro::completions::hint(shell, &name).trim_end());
    Ok(())
}

/// Discovers and loads the configuration, opens the vault, and creates the
/// output directory when one is given. Shared by `clean` and `review`, which
/// otherwise repeated it verbatim.
fn prepare(
    store: &StoreArgs,
    config_path: Option<&Path>,
    output: Option<&Path>,
) -> Result<(Config, Vault)> {
    let config_path = match config_path {
        Some(path) => Some(path.to_path_buf()),
        None => Config::discover_from_cwd(),
    };
    let config = Config::load(config_path.as_deref())?;
    let vault = store.open()?;

    if let Some(dir) = output {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating output directory {}", dir.display()))?;
    }
    Ok((config, vault))
}

/// Guards against two inputs sharing one output.
///
/// The output is named after the stem, so `contract.txt` and `contract.md`
/// both want `contract.clean.md`; writing both would silently lose one. This
/// upfront pass catches duplicate inputs and exact-name collisions before any
/// work is done; everything it cannot see, such as workbook sheet fragments,
/// case folds, and aliased spellings of one path, is caught against the
/// destinations actually claimed during the run by
/// [`oboro::review::WrittenOutputs`].
fn ensure_distinct_outputs(inputs: &[oboro::walk::Input], output: Option<&Path>) -> Result<()> {
    let mut seen_inputs = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for input in inputs {
        if !seen_inputs.insert(input.path.clone()) {
            bail!(
                "{} is listed twice; each input is processed once",
                input.path.display()
            );
        }
        if convert::format_of(&input.path) == Some(convert::Format::Xlsx) {
            continue;
        }
        let destination =
            oboro::review::output_path(&input.path, output, input.root.as_deref(), None, None)?;
        if !seen.insert(destination.clone()) {
            bail!(
                "two inputs would both be written to {}; clean them separately \
                 or into different output directories",
                destination.display()
            );
        }
    }
    Ok(())
}

/// A reader such as `head` closing the pipe early is a normal way to stop, not
/// an error to report, so that one case is swallowed.
///
/// The default `print!` and `println!` panic on a closed pipe, reporting a
/// crash for something the user asked for.
fn ignore_broken_pipe(result: std::io::Result<()>) -> Result<()> {
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => result.context("writing to standard output"),
    }
}

/// Writes cleaned or restored text to standard output.
fn print_stdout(text: &str) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    ignore_broken_pipe(out.write_all(text.as_bytes()).and_then(|()| out.flush()))
}

/// Whether `clean` reads standard input rather than the named paths.
///
/// `-` is the conventional spelling. A bare `oboro clean` in a pipeline reads
/// standard input too, since a caller holding text in memory, such as an agent
/// hook, has no path to name. With a terminal on the other end there is
/// nothing to read, so the missing argument is reported instead of the command
/// hanging on an empty prompt.
fn reads_stdin(files: &[PathBuf]) -> Result<bool> {
    if files.iter().any(|file| file.as_os_str() == "-") {
        if files.len() > 1 {
            bail!("`-` reads standard input and cannot be combined with file paths");
        }
        return Ok(true);
    }
    if !files.is_empty() {
        return Ok(false);
    }
    if std::io::stdin().is_terminal() {
        bail!("no input given; pass a file or directory, or pipe text in on standard input");
    }
    Ok(true)
}

/// Whether `restore` reads standard input rather than a named file.
///
/// The spellings mirror `clean`: an explicit `-`, or no argument at all in a
/// pipeline. See [`reads_stdin`], which cannot be shared here: it arbitrates
/// between several paths, and `restore` takes one.
fn restore_reads_stdin(file: Option<&Path>) -> Result<bool> {
    match file {
        Some(path) => Ok(path.as_os_str() == "-"),
        None if std::io::stdin().is_terminal() => {
            bail!("no input given; pass a file, or pipe text in on standard input")
        }
        None => Ok(true),
    }
}

/// Reads all of standard input as text, refusing anything that is not UTF-8
/// rather than mangling it into output that looks sanitised.
fn read_stdin() -> Result<String> {
    std::io::read_to_string(std::io::stdin()).context("reading standard input")
}

/// Cleans standard input to standard output.
///
/// Standard input has no path to walk and no extension to sniff, so it goes
/// straight to [`pipeline::clean`], bypassing `walk::resolve` and
/// `convert::read`. It is normalised through [`convert::tidy`] all the same, so
/// a piped document and the same document on disk clean identically.
///
/// Filename redaction does not apply: there is no name to redact.
fn clean_stdin(store: &StoreArgs, config_path: Option<&Path>) -> Result<()> {
    let text = read_stdin()?;
    let (config, mut vault) = prepare(store, config_path, None)?;
    let detector = Detector::new(&config)?;
    let report = pipeline::clean(&convert::tidy(&text), &detector, &mut vault)?;
    print_stdout(&report.text)
}

/// Answers a `PostToolUse` hook, replacing the tool's result with a cleaned
/// one so the model reads placeholders rather than values.
///
/// Never returns an error. A `PostToolUse` reply is only honoured when the
/// process exits 0, and the tool has already run by the time this is called, so
/// exiting non-zero would leave the raw result in place: precisely the leak
/// this exists to stop. Every failure therefore answers with a notice in place
/// of the result and a `block` decision, which is what failing closed means for
/// an event that cannot be blocked.
fn hook_post_tool_use(store: &StoreArgs) -> Result<()> {
    match clean_tool_result(store) {
        Ok(Some(cleaned)) => print_stdout(&hook_reply(&cleaned)),
        // Nothing to replace: an empty reply is how a hook says it changed
        // nothing.
        Ok(None) => Ok(()),
        Err(error) => print_stdout(&withheld_reply(&format!("{error:#}"))),
    }
}

/// Cleans the `tool_result` in the payload on standard input, or `None` when
/// the payload carries no result to clean.
fn clean_tool_result(store: &StoreArgs) -> Result<Option<String>> {
    let payload: serde_json::Value =
        serde_json::from_str(&read_stdin()?).context("parsing the hook payload as JSON")?;
    let result = match payload.get("tool_result") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(result) => result,
    };

    let (config, mut vault) = prepare(store, None, None)?;
    let detector = Detector::new(&config)?;

    // A string result is the common case and stays a string. Anything else is
    // walked, so a tool answering with an object has every string in it cleaned
    // and keeps the shape the model has to read. Object keys are left alone:
    // renaming them would change what the tool said, so a result keyed by a
    // path still shows that path.
    match result {
        serde_json::Value::String(text) => {
            Ok(Some(pipeline::clean(text, &detector, &mut vault)?.text))
        }
        other => {
            let mut cleaned = other.clone();
            clean_json_strings(&mut cleaned, &detector, &mut vault)?;
            Ok(Some(
                serde_json::to_string(&cleaned).context("re-encoding the cleaned tool result")?,
            ))
        }
    }
}

/// Cleans every string in `value`, in place, leaving object keys untouched.
fn clean_json_strings(
    value: &mut serde_json::Value,
    detector: &Detector,
    vault: &mut Vault,
) -> Result<()> {
    match value {
        serde_json::Value::String(text) => {
            *text = pipeline::clean(text, detector, vault)?.text;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                clean_json_strings(item, detector, vault)?;
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values_mut() {
                clean_json_strings(field, detector, vault)?;
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(())
}

/// The reply that replaces a tool's result with `cleaned`.
fn hook_reply(cleaned: &str) -> String {
    let reply = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": cleaned,
        }
    });
    format!("{reply}\n")
}

/// The reply that withholds a tool's result because cleaning it failed.
///
/// What the model is told is deliberately vague. The failure it reports carries
/// the context Oboro attaches to its errors, which is often a path, and a path
/// is one of the things a vault redacts; sending it to the model to explain a
/// withheld result would leak by another route. So the detail goes to the user
/// through `systemMessage`, and the model is told only that the result was
/// withheld.
fn withheld_reply(reason: &str) -> String {
    let reply = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput":
                "[oboro withheld this tool result: it could not be anonymised]",
        },
        "decision": "block",
        "reason": "oboro could not anonymise this tool result, so it was withheld. \
                   The reason was reported to the user, who has to resolve it before \
                   this tool can be used again.",
        "systemMessage": format!("oboro withheld a tool result: {reason}"),
    });
    format!("{reply}\n")
}

/// Answers a `PreToolUse` hook, putting real values back into the tool's
/// arguments so a placeholder the model echoed back never reaches a file.
///
/// Never returns an error, for the same reason as [`hook_post_tool_use`]: the
/// reply is only honoured on exit 0. Failing closed means the opposite decision
/// here, though. The tool has not run yet, and letting it run would write
/// `[[PHONE_1]]` into the user's file, so the call is denied instead.
fn hook_pre_tool_use(store: &StoreArgs) -> Result<()> {
    match restore_tool_input(store) {
        Ok(Some(restored)) => print_stdout(&restored),
        // Nothing held a placeholder, so the arguments the tool already has are
        // the right ones and an empty reply leaves them alone.
        Ok(None) => Ok(()),
        Err(error) => print_stdout(&refused_reply(&format!("{error:#}"))),
    }
}

/// Restores every placeholder in the payload's `tool_input`, returning the
/// reply to write, or `None` when nothing changed.
fn restore_tool_input(store: &StoreArgs) -> Result<Option<String>> {
    let payload: serde_json::Value =
        serde_json::from_str(&read_stdin()?).context("parsing the hook payload as JSON")?;
    let Some(input) = payload.get("tool_input") else {
        return Ok(None);
    };

    let vault = store.open()?;
    // Every string in the arguments, not a list of fields per tool: `Write`
    // carries the text in `content`, `Edit` in `old_string` and `new_string`,
    // and a tool added later will carry it somewhere else again. Restoring
    // rewrites only the `[[TAG_n]]` shape, so a string holding no placeholder
    // comes back unchanged whatever field it sits in.
    let mut restored = input.clone();
    let report = restore_json_strings(&mut restored, &vault)?;

    if report.restored == 0 && report.unknown == 0 {
        return Ok(None);
    }
    Ok(Some(restored_reply(&restored, report.unknown)))
}

/// What restoring a set of arguments came to.
struct RestoreTally {
    restored: usize,
    unknown: usize,
}

/// Restores every string in `value`, in place, leaving object keys untouched.
fn restore_json_strings(value: &mut serde_json::Value, vault: &Vault) -> Result<RestoreTally> {
    let mut tally = RestoreTally {
        restored: 0,
        unknown: 0,
    };
    match value {
        serde_json::Value::String(text) => {
            let report = pipeline::restore(text, vault)?;
            *text = report.text;
            tally.restored += report.restored;
            tally.unknown += report.unknown;
        }
        serde_json::Value::Array(items) => {
            for item in items {
                let nested = restore_json_strings(item, vault)?;
                tally.restored += nested.restored;
                tally.unknown += nested.unknown;
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values_mut() {
                let nested = restore_json_strings(field, vault)?;
                tally.restored += nested.restored;
                tally.unknown += nested.unknown;
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(tally)
}

/// The reply that replaces a tool's arguments with restored ones.
///
/// A placeholder this vault never issued is left as it is and reported, which is
/// what `restore` does with a document. It is more likely something the model
/// invented than a mapping to recover, and refusing the write over it would
/// block work on a guess.
fn restored_reply(input: &serde_json::Value, unknown: usize) -> String {
    let mut reply = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": input,
        }
    });
    if unknown > 0 {
        reply["systemMessage"] = serde_json::json!(format!(
            "oboro left {unknown} unknown placeholder(s) in place: this vault never issued them"
        ));
    }
    format!("{reply}\n")
}

/// The reply that refuses a tool call because its arguments could not be
/// restored.
///
/// The model is told to stop rather than told why in detail, as with a withheld
/// result: the reason carries the context Oboro attaches to its errors, which is
/// often a path.
fn refused_reply(reason: &str) -> String {
    let reply = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason":
                "oboro could not put the real values back into these arguments, so the \
                 call was refused rather than write placeholders into a file. The reason \
                 was reported to the user, who has to resolve it first.",
        },
        "systemMessage": format!("oboro refused a tool call: {reason}"),
    });
    format!("{reply}\n")
}

fn clean(
    files: &[PathBuf],
    recursive: bool,
    output: Option<&Path>,
    to_stdout: bool,
    store: &StoreArgs,
    config_path: Option<&Path>,
) -> Result<()> {
    if reads_stdin(files)? {
        if output.is_some() {
            bail!(
                "--output names a directory for files written alongside their inputs; \
                 standard input has no name, and its cleaned text goes to standard output"
            );
        }
        return clean_stdin(store, config_path);
    }
    let resolved = oboro::walk::resolve(files, recursive)?;
    if to_stdout && resolved.inputs.len() > 1 {
        bail!("--stdout takes a single file; pass one file or use --output");
    }
    if !to_stdout {
        ensure_distinct_outputs(&resolved.inputs, output)?;
    }
    if resolved.skipped > 0 {
        oboro::note!("{} unsupported file(s) skipped", resolved.skipped);
    }

    let (config, mut vault) = prepare(store, config_path, output)?;
    // Built once, so a multi-file run loads the recognition model a single
    // time instead of on every file.
    let detector = Detector::new(&config)?;

    // Destinations written this run, catching per-sheet and aliased-path
    // collisions that the input-level guard in `ensure_distinct_outputs`
    // cannot see.
    let mut written = oboro::review::WrittenOutputs::new();
    for input in &resolved.inputs {
        let file = &input.path;
        let parts = convert::read(file, &config.ocr_languages)?.into_parts();

        if to_stdout {
            // A workbook maps to one file per sheet, which a single stream
            // cannot represent; a lone sheet is unambiguous.
            if parts.len() > 1 {
                bail!(
                    "--stdout cannot represent {}: the workbook holds {} non-empty \
                     sheets, each written to its own file; use --output",
                    file.display(),
                    parts.len()
                );
            }
            let report = pipeline::clean(&parts[0].1, &detector, &mut vault)?;
            print_stdout(&report.text)?;
            continue;
        }

        let stem = if config.redact_filenames {
            Some(oboro::review::redacted_stem(file, &detector, &mut vault)?)
        } else {
            None
        };
        let mut namer = oboro::review::SheetNamer::new();
        for (sheet, text) in parts {
            let fragment = match &sheet {
                Some((index, name)) => Some(namer.fragment(
                    name,
                    *index,
                    config.redact_filenames,
                    &detector,
                    &mut vault,
                )?),
                None => None,
            };
            let destination = oboro::review::output_path(
                file,
                output,
                input.root.as_deref(),
                stem.as_deref(),
                fragment.as_deref(),
            )?;
            // Claimed before the body is cleaned, so no placeholder is
            // allocated for values that are never written anywhere.
            written.claim(&destination)?;
            let report = pipeline::clean(&text, &detector, &mut vault)?;
            oboro::review::write_output(&destination, &report.text)?;
            written.record(&destination)?;
            oboro::note!(
                "{} -> {} ({} replaced{})",
                file.display(),
                destination.display(),
                report.replaced,
                summarise(&report.by_tag)
            );
        }
    }

    Ok(())
}

fn review(
    files: &[PathBuf],
    recursive: bool,
    output: Option<&Path>,
    store: &StoreArgs,
    config_path: Option<&Path>,
) -> Result<()> {
    let resolved = oboro::walk::resolve(files, recursive)?;
    ensure_distinct_outputs(&resolved.inputs, output)?;
    let (config, mut vault) = prepare(store, config_path, output)?;
    oboro::review::run(
        &resolved.inputs,
        resolved.skipped,
        &config,
        &mut vault,
        output,
    )
}

fn summarise(by_tag: &std::collections::BTreeMap<String, usize>) -> String {
    if by_tag.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = by_tag
        .iter()
        .map(|(tag, count)| format!("{tag} {count}"))
        .collect();
    format!(": {}", parts.join(", "))
}

/// Puts real values back into a document, in place or on standard output.
///
/// Standard input has no path to rewrite, so piped text always leaves on
/// standard output, whatever `--stdout` says.
fn restore(file: Option<&Path>, to_stdout: bool, store: &StoreArgs) -> Result<()> {
    let source = if restore_reads_stdin(file)? {
        None
    } else {
        file
    };
    let text = match source {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => read_stdin()?,
    };
    let vault = store.open()?;
    let report = pipeline::restore(&text, &vault)?;

    match source {
        Some(path) if !to_stdout => {
            oboro::claude::write_atomic(path, report.text.as_bytes())
                .with_context(|| format!("writing {}", path.display()))?;
            oboro::note!("{}: {} restored", path.display(), report.restored);
        }
        _ => print_stdout(&report.text)?,
    }

    if report.unknown > 0 {
        oboro::note!(
            "warning: {} placeholder(s) are unknown to this vault and were left in place",
            report.unknown
        );
    }
    Ok(())
}

fn map_list(reveal: bool, store: &StoreArgs) -> Result<()> {
    let vault = store.open()?;
    let entries = vault.entries()?;
    if entries.is_empty() {
        oboro::note!("the vault is empty");
        return Ok(());
    }

    let mut listing = String::new();
    for entry in entries {
        let line = if reveal {
            // A listed placeholder with no stored value means the row was lost
            // or the database was tampered with; an empty column would hide it.
            let value = vault.value_for(&entry.tag, entry.seq)?.ok_or_else(|| {
                anyhow!(
                    "the vault lists {} but holds no value for it; the database may be corrupt",
                    entry.placeholder()
                )
            })?;
            format!("{}\t{}\t{}", entry.placeholder(), entry.created_at, value)
        } else {
            format!("{}\t{}", entry.placeholder(), entry.created_at)
        };
        listing.push_str(&line);
        listing.push('\n');
    }
    print_stdout(&listing)?;
    if !reveal {
        oboro::note!("values hidden; pass --reveal to print them");
    }
    Ok(())
}

fn map_purge(confirmed: bool, store: &StoreArgs) -> Result<()> {
    if !confirmed {
        bail!(
            "purging deletes every mapping and makes existing sanitised output unrecoverable; \
             pass --yes to confirm"
        );
    }
    let vault = store.open()?;
    let removed = vault.purge()?;
    oboro::note!("removed {removed} mapping(s)");
    Ok(())
}

/// Writes the skill an agent reads to understand the hooks' placeholders, and
/// with `--with-hooks` the hooks that make it worth reading.
///
/// The two halves are one decision: a skill without the hooks describes
/// placeholders the agent will never see, and hooks without the skill leave it
/// guessing at the ones it does. Both are planned before either is written, so
/// the refusals that can be seen up front, a symbolic link or a settings file
/// that cannot be merged into, stop the pair rather than leaving half of it
/// installed. A write that fails partway through is not covered by that and
/// says which half it managed.
fn skill_install(scope: Option<Scope>, dry_run: bool, force: bool, with_hooks: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading the working directory")?;
    let scope = match (scope, with_hooks) {
        (Some(scope), _) => scope,
        (None, false) => ask_for_scope(&cwd, "skill", &[oboro::skill::path])?,
        (None, true) => ask_for_scope(
            &cwd,
            "skill and hooks",
            &[oboro::skill::path, oboro::hooks::settings_path],
        )?,
    };
    // The plans are made once and then carried out, so what is named here is by
    // construction what happens, whether or not `--dry-run` stops it.
    let plan = oboro::skill::plan(scope, &cwd, force)?;
    let hooks = with_hooks
        .then(|| oboro::hooks::plan(scope, &cwd))
        .transpose()?;

    match &plan {
        Plan::Write(path) => oboro::note!("writing {}", path.display()),
        Plan::Keep(path) => oboro::note!("{} already holds this skill", path.display()),
        Plan::Propose {
            installed,
            proposed,
        } => oboro::note!(
            "{} differs from the skill this build carries, so it is left alone.\n\
             Writing {} instead: compare the two, or re-run with --force.",
            installed.display(),
            proposed.display()
        ),
    }
    if let Some(hooks) = &hooks {
        note_if_a_plugin_carries_the_hooks(&cwd);
        describe_hook_plan(hooks);
    }

    if dry_run {
        oboro::note!("--dry-run: nothing was written");
        if let Some(hooks) = &hooks {
            oboro::note!("The settings would read:");
            return print_stdout(&hooks.rendered()?);
        }
        return Ok(());
    }

    if matches!(oboro::skill::install(plan)?, Plan::Write(_)) {
        oboro::note!("installed the skill for {}", describe_scope(scope));
    }
    if let Some(hooks) = hooks {
        install_hook_plan(hooks, scope)?;
    }
    Ok(())
}

/// Names both hooks in the user's settings, so the agent path is covered
/// without anyone pasting JSON.
fn hook_install(scope: Option<Scope>, dry_run: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("reading the working directory")?;
    let scope = match scope {
        Some(scope) => scope,
        None => ask_for_scope(&cwd, "hooks", &[oboro::hooks::settings_path])?,
    };

    let plan = oboro::hooks::plan(scope, &cwd)?;
    note_if_a_plugin_carries_the_hooks(&cwd);
    describe_hook_plan(&plan);
    if dry_run {
        oboro::note!("--dry-run: nothing was written. The settings would read:");
        return print_stdout(&plan.rendered()?);
    }

    install_hook_plan(plan, scope)
}

/// Says so when a plugin already carries the hooks about to be named.
///
/// Both copies run on every matching tool call, cleaning what is already
/// cleaned and restoring what is already restored, for as long as both are
/// installed. `doctor` reports the collision afterwards; this is the moment it
/// would be created, so it is said here rather than left to be discovered.
fn note_if_a_plugin_carries_the_hooks(cwd: &Path) {
    for plugin in oboro::hooks::enabled_plugins_from(cwd) {
        oboro::note!(
            "note: the {} plugin is enabled in {} and carries both hooks already; \
             naming them here as well runs each of them twice",
            plugin.key,
            plugin.file.display()
        );
    }
}

/// Says what installing the hooks would do to each event, and to the file.
fn describe_hook_plan(plan: &oboro::hooks::Plan) {
    for (event, change) in &plan.changes {
        match change {
            Change::Add(matcher) => oboro::note!("{event:<11} adding, matched against {matcher}"),
            Change::Keep(matcher) => oboro::note!(
                "{event:<11} already named, matched against {}; left as you wrote it",
                matcher.as_deref().unwrap_or("every tool")
            ),
        }
    }
    if plan.writes() {
        oboro::note!("writing {}", plan.file.display());
    } else {
        oboro::note!("{} already names both halves", plan.file.display());
    }
}

/// Carries out a hook plan and reports what it did.
fn install_hook_plan(plan: oboro::hooks::Plan, scope: Scope) -> Result<()> {
    let written = plan.writes();
    let file = plan.file.clone();
    oboro::hooks::install(plan)?;
    if written {
        oboro::note!(
            "installed for {}: {}",
            describe_scope(scope),
            file.display()
        );
    }
    if written && !oboro::hooks::program_is_reachable("oboro") {
        // A hook naming a binary the agent cannot find is configured and
        // useless, and it fails closed on every matching tool call.
        oboro::note!(
            "warning: `oboro` is not on PATH, so the hooks just named cannot run; \
             put the binary on PATH, or edit the commands to give its full path"
        );
    }
    Ok(())
}

/// The scope the flags named, or `None` when neither did and it has to be
/// asked for.
fn chosen_scope(project: bool, user: bool) -> Option<Scope> {
    match (project, user) {
        (true, _) => Some(Scope::Project),
        (_, true) => Some(Scope::User),
        _ => None,
    }
}

/// Asks which scope to install `what` into, when neither flag named one.
///
/// The paths are shown rather than named, because the difference that matters
/// is which agent sessions will read the file, and a path answers that where
/// "project" and "user" do not. The installers write different files, so each
/// passes its own way of resolving one, and installing both halves at once
/// passes both: what a scope means is exactly the files it would write.
/// How wide the column naming what a scope covers is, in [`ask_for_scope`].
const COVERS: usize = 14;

/// What precedes that column: the choice's number and the two spaces after it.
const NUMBER: usize = 3;

fn ask_for_scope(
    cwd: &Path,
    what: &str,
    paths_for: &[fn(Scope, &Path) -> Result<PathBuf>],
) -> Result<Scope> {
    if !std::io::stdin().is_terminal() {
        bail!(
            "there is no terminal to ask which scope to install into; \
             pass --project for this project or --user for every project"
        );
    }

    for (choice, scope) in SCOPES.iter().enumerate() {
        let covers = describe_scope(*scope);
        for (line, path_for) in paths_for.iter().enumerate() {
            // The scope is named once, beside the first of its paths; the rest
            // line up underneath it rather than repeating it.
            let label = if line == 0 {
                format!("{}  {covers:<COVERS$}", choice + 1)
            } else {
                " ".repeat(NUMBER + COVERS)
            };
            match path_for(*scope, cwd) {
                Ok(path) => oboro::note!("{label} {}", path.display()),
                Err(error) => oboro::note!("{label} unavailable: {error:#}"),
            }
        }
    }
    oboro::note!("Install the Oboro {what} where? [1/2]");

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading your answer")?;
    match answer.trim() {
        "1" => Ok(Scope::Project),
        "2" => Ok(Scope::User),
        other => bail!("{other:?} is neither 1 nor 2; nothing was written"),
    }
}

/// How a scope is described in a message, in terms of what it covers.
fn describe_scope(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "this project",
        Scope::User => "every project",
    }
}

/// Describes the phone regions in force and where they came from, since an
/// unset `regions` follows the locale and that is worth seeing.
fn describe_regions(config: &Config) -> String {
    let codes = config.region_codes();
    match (&config.regions_source, codes.is_empty()) {
        (_, true) => "none (international + numbers only)".to_owned(),
        (RegionSource::Configured, _) => {
            format!("{} (from {})", codes.join(", "), config::CONFIG_FILE)
        }
        (RegionSource::Locale(variable), _) => {
            format!("{} (from ${variable})", codes.join(", "))
        }
        (RegionSource::Unknown, _) => codes.join(", "),
    }
}

/// Serves the Model Context Protocol on standard input and output.
///
/// The configuration is resolved here rather than inside the server so the
/// startup line can name the file it came from. A silent default is the thing
/// that line guards against, so it has to be written where the path is still
/// in hand.
fn mcp(
    config_path: Option<&Path>,
    roots: &[PathBuf],
    unconfined: bool,
    store: &StoreArgs,
) -> Result<()> {
    // Settled before anything is opened, so a server that will refuse every
    // call says so at once rather than on the model's first request.
    let roots = match (roots.is_empty(), unconfined) {
        (true, false) => bail!(
            "name at least one directory the `clean` tool may read within, with --root, \
             or pass --unconfined to let it read any file you can read. \
             The caller over this protocol is a model rather than you, so reading the \
             whole disk is not something to arrive at by leaving a flag off."
        ),
        (_, true) => oboro::mcp::Roots::Unconfined,
        (false, false) => oboro::mcp::Roots::within(roots)?,
    };
    // `--root /` parses and confines nothing. Saying so is cheap, and the
    // alternative is a server that looks bounded and is not.
    if roots_name_the_whole_filesystem(&roots) {
        oboro::note!(
            "oboro mcp: warning: a root of / confines nothing; \
             name the directories the work actually needs"
        );
    }

    let resolved = match config_path {
        Some(path) => Some(path.to_path_buf()),
        None => Config::discover_from_cwd(),
    };
    match &resolved {
        Some(path) => oboro::note!("oboro mcp: configuration {}", path.display()),
        None => oboro::note!(
            "oboro mcp: no oboro.toml found from {}; running on the defaults, \
             with no denylist and no custom patterns. Pass --config or set OBORO_CONFIG.",
            std::env::current_dir().map_or_else(
                |_| "the working directory".to_owned(),
                |dir| dir.display().to_string()
            )
        ),
    }

    let config = Config::load(resolved.as_deref())?;
    oboro::note!("oboro mcp: detectors {}", describe_detectors(&config));
    match &roots {
        oboro::mcp::Roots::Unconfined => oboro::note!(
            "oboro mcp: reading anywhere you can read, because --unconfined was passed"
        ),
        oboro::mcp::Roots::Within(within) => {
            for root in within {
                oboro::note!("oboro mcp: root {}", root.display());
            }
        }
    }

    let vault = store.open()?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    oboro::mcp::serve(stdin.lock(), stdout.lock(), &config, vault, roots)
}

/// Whether any root is the filesystem root, which admits everything.
fn roots_name_the_whole_filesystem(roots: &oboro::mcp::Roots) -> bool {
    match roots {
        oboro::mcp::Roots::Unconfined => false,
        oboro::mcp::Roots::Within(within) => within.iter().any(|root| root.parent().is_none()),
    }
}

/// Which detectors are configured and installed.
///
/// Configured and installed, not loaded: the recognition model is built on the
/// first call rather than at startup, and its outcome is per-call, so whether
/// it loads is not knowable here.
fn describe_detectors(config: &Config) -> String {
    #[cfg(feature = "ner")]
    {
        if config.ner_enabled && oboro::models::is_installed().unwrap_or(false) {
            return "rules and the recognition model".to_owned();
        }
        if config.ner_enabled {
            return "rules only; the recognition model is enabled but not installed, \
                    so names are matched from the denylist alone"
                .to_owned();
        }
        "rules only; the recognition model is disabled in the configuration".to_owned()
    }
    #[cfg(not(feature = "ner"))]
    {
        let _ = config;
        "rules only; this build has no recognition model compiled in".to_owned()
    }
}

fn doctor(store: &StoreArgs) -> Result<()> {
    use std::fmt::Write as _;

    let (db, key) = store.paths()?;
    let mut report = String::new();
    writeln!(report, "vault:      {}", db.display())?;
    writeln!(report, "key:        {}", key.display())?;

    #[cfg(unix)]
    for path in [&db, &key] {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            let state = if mode == 0o600 {
                "ok"
            } else {
                "too permissive"
            };
            writeln!(report, "  {} mode {mode:04o} ({state})", path.display())?;
        }
    }

    let config_path = Config::discover_from_cwd();
    match &config_path {
        Some(path) => writeln!(report, "config:     {}", path.display())?,
        None => writeln!(report, "config:     none found (using defaults)")?,
    }

    // A configuration that will not load is one of the reasons to run this
    // command, so what has been gathered so far, including the path of the
    // offending file, is written before the error is reported.
    let config = match Config::load(config_path.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            print_stdout(&report)?;
            return Err(error);
        }
    };
    writeln!(report, "regions:    {}", describe_regions(&config))?;
    writeln!(report, "allowlist:  {} entr(y/ies)", config.allowlist.len())?;
    writeln!(report, "denylist:   {} term(s)", config.denylist.len())?;
    writeln!(report, "patterns:   {} custom", config.patterns.len())?;
    writeln!(
        report,
        "filenames:  {}",
        if config.redact_filenames {
            "redacted"
        } else {
            "kept"
        }
    )?;
    writeln!(report, "formats:    {}", convert::supported().join(", "))?;
    writeln!(
        report,
        "ocr:        {}",
        if convert::ocr_available() {
            "available"
        } else {
            "not compiled in; images and scanned PDFs cannot be read"
        }
    )?;
    if convert::ocr_available() {
        writeln!(
            report,
            "ocr langs:  {}",
            if config.ocr_languages.is_empty() {
                "not set; whatever Tesseract has installed".to_owned()
            } else {
                config.ocr_languages.join(", ")
            }
        )?;
    }
    #[cfg(feature = "ner")]
    {
        let installed = oboro::models::is_installed().unwrap_or(false);
        writeln!(
            report,
            "model:      {}",
            if installed {
                "installed".to_owned()
            } else {
                format!(
                    "not installed; run `oboro models pull` (about {} MB)",
                    oboro::models::download_bytes() / 1_048_576
                )
            }
        )?;
    }
    #[cfg(not(feature = "ner"))]
    writeln!(
        report,
        "model:      not compiled in; names are matched from the denylist only"
    )?;
    #[cfg(feature = "ner")]
    writeln!(
        report,
        "network:    only `models pull`, and only when you run it"
    )?;
    // Without that command there is nothing in this build that can open a
    // socket, and saying otherwise would overstate what it does.
    #[cfg(not(feature = "ner"))]
    writeln!(report, "network:    never contacted")?;
    // One working directory for both, so the two halves of the agent report
    // cannot describe different places. The plugin is resolved once here for
    // the same reason: it carries a skill and hooks, so a report where one half
    // knows about it and the other does not would contradict itself.
    let cwd = std::env::current_dir().context("reading the working directory")?;
    let plugins = oboro::hooks::enabled_plugins_from(&cwd);
    write!(report, "{}", describe_hooks(&cwd, &plugins)?)?;
    write!(report, "{}", describe_skill(&cwd, &plugins)?)?;
    write!(report, "{}", describe_completions(&mut Cli::command())?)?;
    print_stdout(&report)
}

/// Reports which agent hooks are installed, so a user can check rather than
/// assume.
///
/// Both halves are reported even when neither is installed: a user who has only
/// the cleaning half is in the worse position of the two, with the model writing
/// placeholders into their files, and silence would not tell them.
///
/// The plugin is reported first, since it carries hooks of its own that no
/// settings file names. Without that line the advice under each event would be
/// wrong for anyone who installed that way, and following it would leave them
/// running both copies.
fn describe_hooks(cwd: &Path, plugins: &[EnabledPlugin]) -> Result<String> {
    use std::fmt::Write as _;

    let installed = oboro::hooks::installed_from(cwd);
    let mut report = String::new();

    for plugin in plugins {
        writeln!(
            report,
            "plugin      {} ({}, enabled)",
            plugin.file.display(),
            plugin.key
        )?;
    }

    for event in oboro::hooks::EVENTS {
        let name = event.name;
        let found: Vec<_> = installed.iter().filter(|hook| hook.event == name).collect();
        if found.is_empty() {
            let advice = if plugins.is_empty() {
                "not installed; run `oboro hook install`"
            } else {
                "not in your settings; the plugin carries it"
            };
            writeln!(report, "{name:<11} {advice}")?;
            continue;
        }
        for hook in found {
            let matcher = hook.matcher.as_deref().unwrap_or("every tool");
            let reachable = if oboro::hooks::program_is_reachable(&hook.command) {
                "reachable"
            } else {
                "NOT REACHABLE"
            };
            writeln!(
                report,
                "{name:<11} {} ({matcher}, {reachable})",
                hook.file.display()
            )?;
        }
    }
    Ok(report)
}

/// Reports where the skill is installed, for the same reason the hooks are
/// reported: a user cannot otherwise tell whether the agent reading their
/// placeholders has ever been told what they are.
///
/// Both scopes are listed whatever their state. An `edited` copy is the one
/// worth noticing, since that is the agent being taught something this build no
/// longer does.
///
/// A plugin carries a skill of its own, in its own files, so `not installed` on
/// both scopes is true of these paths and misleading about the agent. It is
/// said differently when one is enabled, for the reason the hooks are.
fn describe_skill(cwd: &Path, plugins: &[EnabledPlugin]) -> Result<String> {
    use std::fmt::Write as _;

    let mut report = String::new();
    let missing = if plugins.is_empty() {
        "not installed"
    } else {
        "not installed here; the plugin carries its own"
    };

    for scope in SCOPES {
        let Ok(path) = oboro::skill::path(scope, cwd) else {
            writeln!(
                report,
                "skill       {}: no home directory",
                describe_scope(scope)
            )?;
            continue;
        };
        let state = match oboro::skill::status(&path) {
            Status::Missing => missing,
            Status::Current => "current",
            Status::Edited => "edited; `oboro skill install` will propose the new text",
            Status::Unreadable => "UNREADABLE",
        };
        writeln!(report, "skill       {} ({state})", path.display())?;
    }
    Ok(report)
}

/// How wide the label column is, so a continuation line lines up under what it
/// continues rather than under the label.
const LABEL: usize = 12;

/// Reports whether the completion scripts on disk still match this binary.
///
/// A completion script is a copy of the command surface at the moment it was
/// generated, so a release that adds a command leaves the script offering the
/// old set and nothing says so. Regenerating and comparing byte for byte
/// answers that exactly, with no version to parse and nothing to keep in step.
///
/// Every conventional path is checked whatever `$SHELL` says. Reading that
/// variable is the obvious way to pick one and is wrong often enough to matter,
/// and a path that does not exist is not reported anyway, so checking all of
/// them costs a line only for scripts that are really there.
///
/// Absence is reported rather than passed over in silence, as the hooks and the
/// skill are: a report that says nothing cannot be told from one that found
/// nothing to say.
fn describe_completions(command: &mut clap::Command) -> Result<String> {
    use std::fmt::Write as _;

    let name = command.get_name().to_owned();
    let mut report = String::new();
    let mut found = false;

    for location in oboro::completions::conventional_paths(&name) {
        let Ok(installed) = std::fs::read(&location.path) else {
            // Either nothing is there, which is the ordinary case and not worth
            // a line, or it cannot be read, which the next stat tells apart.
            if location.path.exists() {
                found = true;
                writeln!(
                    report,
                    "completion  {} (UNREADABLE)",
                    location.path.display()
                )?;
            }
            continue;
        };
        found = true;
        if installed == oboro::completions::script(location.shell, command).as_bytes() {
            writeln!(report, "completion  {} (current)", location.path.display())?;
            continue;
        }
        writeln!(report, "completion  {} (stale)", location.path.display())?;
        writeln!(
            report,
            "{:LABEL$}{name} completions {} > {}",
            "",
            location.shell,
            location.path.display()
        )?;
    }

    if !found {
        writeln!(
            report,
            "completion  not installed; run `{name} completions <shell>`"
        )?;
    }
    Ok(report)
}
