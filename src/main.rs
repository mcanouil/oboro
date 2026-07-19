//! Command line entry point.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use oboro::config::Config;
use oboro::convert;
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
        /// Files to anonymise
        #[arg(required = true, value_name = "FILE")]
        files: Vec<PathBuf>,
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
    /// Report the tool's configuration and environment
    Doctor,
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
    #[arg(long, value_name = "FILE", global = true)]
    vault: Option<PathBuf>,
    /// Encryption key file (defaults to ~/.oboro/key)
    #[arg(long, value_name = "FILE", global = true)]
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
            output,
            stdout,
            config,
        } => clean(&files, output.as_deref(), stdout, store, config.as_deref()),
        Command::Restore { file, stdout } => restore(&file, stdout, store),
        Command::Map { action } => match action {
            MapAction::List { reveal } => map_list(reveal, store),
            MapAction::Purge { yes } => map_purge(yes, store),
        },
        Command::Doctor => doctor(store),
    }
}

fn clean(
    files: &[PathBuf],
    output: Option<&Path>,
    to_stdout: bool,
    store: &StoreArgs,
    config_path: Option<&Path>,
) -> Result<()> {
    if to_stdout && files.len() > 1 {
        bail!("--stdout takes a single file; pass one file or use --output");
    }

    let config_path = match config_path {
        Some(path) => Some(path.to_path_buf()),
        None => Config::discover_from_cwd(),
    };
    let config = Config::load(config_path.as_deref())?;
    let mut vault = store.open()?;

    if let Some(dir) = output {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating output directory {}", dir.display()))?;
    }

    for file in files {
        let text = convert::to_text(file)?;
        let report = pipeline::clean(&text, &config, &mut vault)?;

        if to_stdout {
            print!("{}", report.text);
        } else {
            let destination = output_path(file, output)?;
            std::fs::write(&destination, &report.text)
                .with_context(|| format!("writing {}", destination.display()))?;
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

/// Builds the sanitised output path, `report.docx` becoming `report.clean.md`.
fn output_path(input: &Path, output_dir: Option<&Path>) -> Result<PathBuf> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("{} has no usable file name", input.display()))?;
    let name = format!("{stem}.clean.md");
    Ok(match output_dir {
        Some(dir) => dir.join(name),
        None => input.with_file_name(name),
    })
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
        std::fs::write(file, &report.text)
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
        if reveal {
            let value = vault.value_for(&entry.tag, entry.seq)?.unwrap_or_default();
            writeln!(
                out,
                "{}	{}	{}",
                entry.placeholder(),
                entry.created_at,
                value
            )?;
        } else {
            writeln!(out, "{}	{}", entry.placeholder(), entry.created_at)?;
        }
    }
    if !reveal {
        eprintln!("values hidden; pass --reveal to print them");
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
    println!("formats:    {}", convert::supported().join(", "));
    println!(
        "ocr:        {}",
        if convert::ocr_available() {
            "available"
        } else {
            "not compiled in; images cannot be read"
        }
    );
    println!("network:    never contacted");
    Ok(())
}
