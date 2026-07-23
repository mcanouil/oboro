//! Command line entry point.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};

use oboro::config::Config;
use oboro::convert;
use oboro::detect::Detector;
use oboro::pipeline;
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
    #[command(flatten)]
    store: StoreArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Anonymise files into sanitised markdown
    Clean {
        /// Files or directories to anonymise
        #[arg(required = true, value_name = "PATH")]
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
    },
    /// Put real values back into a model's answer
    Restore {
        /// File containing placeholders
        #[arg(value_name = "FILE")]
        file: PathBuf,
        /// Write to standard output instead of a file
        #[arg(long)]
        stdout: bool,
    },
    /// Inspect or wipe the placeholder mapping
    Map {
        #[command(subcommand)]
        action: MapAction,
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
    },
    /// Report the tool's configuration and environment
    Doctor,
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
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let store = &cli.store;
    match cli.command {
        Command::Clean {
            files,
            recursive,
            output,
            stdout,
            config,
        } => clean(
            &files,
            recursive,
            output.as_deref(),
            stdout,
            store,
            config.as_deref(),
        ),
        Command::Restore { file, stdout } => restore(&file, stdout, store),
        Command::Map { action } => match action {
            MapAction::List { reveal } => map_list(reveal, store),
            MapAction::Purge { yes } => map_purge(yes, store),
        },
        #[cfg(feature = "ner")]
        Command::Models { action } => match action {
            ModelAction::Pull => oboro::models::pull(),
            ModelAction::Status => {
                print!("{}", oboro::models::status()?);
                Ok(())
            }
        },
        Command::Review {
            files,
            recursive,
            output,
            config,
        } => review(
            &files,
            recursive,
            output.as_deref(),
            store,
            config.as_deref(),
        ),
        Command::Doctor => doctor(store),
    }
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
/// both want `contract.clean.md`; writing both would silently lose one. Caught
/// before anything is written so nothing is half-done. Workbooks are keyed by
/// their base path without a sheet fragment, since sheet names are unknown
/// until read; per-sheet collisions are caught against the destinations
/// actually written in the `clean` loop.
fn ensure_distinct_outputs(inputs: &[oboro::walk::Input], output: Option<&Path>) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for input in inputs {
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

fn clean(
    files: &[PathBuf],
    recursive: bool,
    output: Option<&Path>,
    to_stdout: bool,
    store: &StoreArgs,
    config_path: Option<&Path>,
) -> Result<()> {
    let resolved = oboro::walk::resolve(files, recursive)?;
    if to_stdout && resolved.inputs.len() > 1 {
        bail!("--stdout takes a single file; pass one file or use --output");
    }
    if !to_stdout {
        ensure_distinct_outputs(&resolved.inputs, output)?;
    }
    if resolved.skipped > 0 {
        eprintln!("{} unsupported file(s) skipped", resolved.skipped);
    }

    let (config, mut vault) = prepare(store, config_path, output)?;
    // Built once, so a multi-file run loads the recognition model a single
    // time instead of on every file.
    let detector = Detector::new(&config)?;

    // Destinations written this run, catching per-sheet collisions that the
    // input-level guard in `ensure_distinct_outputs` cannot see.
    let mut written = std::collections::HashSet::new();
    for input in &resolved.inputs {
        let file = &input.path;
        let parts = convert::read(file)?.into_parts();

        if to_stdout {
            // A workbook maps to one file per sheet, which a single stream
            // cannot represent; a lone sheet is unambiguous.
            if parts.len() > 1 {
                bail!(
                    "--stdout cannot represent {}: the workbook holds {} sheets, \
                     each written to its own file; use --output",
                    file.display(),
                    parts.len()
                );
            }
            let report = pipeline::clean(&parts[0].1, &detector, &mut vault)?;
            print!("{}", report.text);
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
            let report = pipeline::clean(&text, &detector, &mut vault)?;
            let destination = oboro::review::output_path(
                file,
                output,
                input.root.as_deref(),
                stem.as_deref(),
                fragment.as_deref(),
            )?;
            if !written.insert(destination.clone()) {
                bail!(
                    "two inputs would both be written to {}; clean them separately \
                     or into different output directories",
                    destination.display()
                );
            }
            oboro::review::write_output(&destination, &report.text)?;
            eprintln!(
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

fn restore(file: &Path, to_stdout: bool, store: &StoreArgs) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let vault = store.open()?;
    let report = pipeline::restore(&text, &vault)?;

    if to_stdout {
        print!("{}", report.text);
    } else {
        write_atomic(file, report.text.as_bytes())
            .with_context(|| format!("writing {}", file.display()))?;
        eprintln!("{}: {} restored", file.display(), report.restored);
    }

    if report.unknown > 0 {
        eprintln!(
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
        eprintln!("the vault is empty");
        return Ok(());
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
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
        // A reader such as `head` closing the pipe early is a normal way to
        // stop, not an error to report.
        if let Err(error) = writeln!(out, "{line}") {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(error).context("writing the mapping listing");
        }
    }
    if !reveal {
        eprintln!("values hidden; pass --reveal to print them");
    }
    Ok(())
}

/// Writes a file by writing a sibling temporary and renaming it into place.
///
/// `restore` overwrites the user's only copy of the answer, so a crash partway
/// through a direct write would lose it. Renaming is atomic on the same
/// filesystem, so the destination is either the old file or the whole new one.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
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

fn map_purge(confirmed: bool, store: &StoreArgs) -> Result<()> {
    if !confirmed {
        bail!(
            "purging deletes every mapping and makes existing sanitised output unrecoverable; \
             pass --yes to confirm"
        );
    }
    let vault = store.open()?;
    let removed = vault.purge()?;
    eprintln!("removed {removed} mapping(s)");
    Ok(())
}

fn doctor(store: &StoreArgs) -> Result<()> {
    let (db, key) = store.paths()?;
    println!("vault:      {}", db.display());
    println!("key:        {}", key.display());

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
            println!("  {} mode {mode:04o} ({state})", path.display());
        }
    }

    let config_path = Config::discover_from_cwd();
    match &config_path {
        Some(path) => println!("config:     {}", path.display()),
        None => println!("config:     none found (using defaults)"),
    }

    let config = Config::load(config_path.as_deref())?;
    println!("region:     {}", config.default_region);
    println!("allowlist:  {} entr(y/ies)", config.allowlist.len());
    println!("denylist:   {} term(s)", config.denylist.len());
    println!("patterns:   {} custom", config.patterns.len());
    println!(
        "filenames:  {}",
        if config.redact_filenames {
            "redacted"
        } else {
            "kept"
        }
    );
    println!("formats:    {}", convert::supported().join(", "));
    println!(
        "ocr:        {}",
        if convert::ocr_available() {
            "available"
        } else {
            "not compiled in; images cannot be read"
        }
    );
    #[cfg(feature = "ner")]
    {
        let installed = oboro::models::is_installed().unwrap_or(false);
        println!(
            "model:      {}",
            if installed {
                "installed".to_owned()
            } else {
                format!(
                    "not installed; run `oboro models pull` (about {} MB)",
                    oboro::models::download_bytes() / 1_048_576
                )
            }
        );
    }
    #[cfg(not(feature = "ner"))]
    println!("model:      not compiled in; names are matched from the denylist only");
    #[cfg(feature = "ner")]
    println!("network:    only `models pull`, and only when you run it");
    // Without that command there is nothing in this build that can open a
    // socket, and saying otherwise would overstate what it does.
    #[cfg(not(feature = "ner"))]
    println!("network:    never contacted");
    Ok(())
}
