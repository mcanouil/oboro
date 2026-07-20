//! Fetching and verifying the local recognition model.
//!
//! `models pull` is the only command in this tool that touches the network,
//! and it does so because the user asked it to. Everything afterwards runs
//! offline against the files it left behind.
//!
//! Downloads are checked against pinned hashes. A model that decides which
//! parts of a document are sensitive is worth verifying: without the check, a
//! compromised mirror could hand over weights that quietly recognise nothing.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

/// Where the weights come from.
///
/// An ONNX export of `GLiNER` multi PII, whose upstream is Apache-2.0. The
/// quantised export is a third of the size of the full one and runs
/// comfortably on a CPU, which matters for a tool meant to run on a laptop
/// beside the documents it reads.
const REPOSITORY: &str = "onnx-community/gliner_multi_pii-v1";
const REVISION: &str = "main";

/// A file to fetch, with the hash it must have once downloaded.
struct Artefact {
    /// Path within the model repository.
    remote: &'static str,
    /// File name on disk.
    local: &'static str,
    sha256: &'static str,
    bytes: u64,
}

const ARTEFACTS: &[Artefact] = &[
    Artefact {
        remote: "onnx/model_quantized.onnx",
        local: "model.onnx",
        sha256: "3efb3b91aef91ae11cd781126133063a33a2ffd7787ec73057bbd57a9781a7ab",
        bytes: 349_120_924,
    },
    Artefact {
        remote: "tokenizer.json",
        local: "tokenizer.json",
        sha256: "914bd3c8fb7b525af9e23b60d0ec7b1248ddb2b99014efd9c02ebeb022f8cab7",
        bytes: 16_331_948,
    },
];

/// Total download size, for telling the user what they are in for.
#[must_use]
pub fn download_bytes() -> u64 {
    ARTEFACTS.iter().map(|artefact| artefact.bytes).sum()
}

/// The directory holding the installed model.
///
/// # Errors
///
/// Returns an error if the home directory cannot be determined.
pub fn directory() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow!("cannot determine the home directory to store the model in"))?;
    Ok(home.join(".oboro").join("models").join("gliner-multi-pii"))
}

/// Whether every artefact is present, without rehashing them.
///
/// # Errors
///
/// Returns an error if the model directory cannot be determined.
pub fn is_installed() -> Result<bool> {
    let dir = directory()?;
    Ok(ARTEFACTS
        .iter()
        .all(|artefact| dir.join(artefact.local).is_file()))
}

/// Paths to the model and its tokenizer, if installed.
///
/// # Errors
///
/// Returns an error if the model is not installed.
pub fn paths() -> Result<(PathBuf, PathBuf)> {
    let dir = directory()?;
    let model = dir.join("model.onnx");
    let tokenizer = dir.join("tokenizer.json");
    if !model.is_file() || !tokenizer.is_file() {
        bail!(
            "the recognition model is not installed; run `oboro models pull` to fetch it \
             (about {} MB, once)",
            download_bytes() / 1_048_576
        );
    }
    Ok((model, tokenizer))
}

/// Reports what is installed, and where.
///
/// # Errors
///
/// Returns an error if the model directory cannot be determined or read.
pub fn status() -> Result<String> {
    let dir = directory()?;
    let mut report = String::new();
    writeln!(report, "model directory: {}", dir.display())?;
    for artefact in ARTEFACTS {
        let path = dir.join(artefact.local);
        let state = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.len() == artefact.bytes => "present".to_owned(),
            Ok(metadata) => format!(
                "wrong size ({} bytes, expected {})",
                metadata.len(),
                artefact.bytes
            ),
            Err(_) => "missing".to_owned(),
        };
        writeln!(report, "  {:<16} {state}", artefact.local)?;
    }
    writeln!(report, "source: {REPOSITORY} at {REVISION}")?;
    Ok(report)
}

/// Downloads any missing artefact and verifies all of them.
///
/// Existing files are re-verified rather than trusted, so an interrupted
/// download cannot leave a truncated model in place that fails later in a
/// confusing way.
///
/// # Errors
///
/// Returns an error if a download fails or a file's hash does not match the
/// pinned value.
pub fn pull() -> Result<()> {
    let dir = directory()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    for artefact in ARTEFACTS {
        let path = dir.join(artefact.local);

        if path.is_file() {
            if verify(&path, artefact.sha256).is_ok() {
                eprintln!("{} already present and verified", artefact.local);
                continue;
            }
            eprintln!(
                "{} is corrupt or incomplete, fetching again",
                artefact.local
            );
            // verify() removes a file that fails, so nothing to clean up.
        }

        let url = format!(
            "https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{}",
            artefact.remote
        );
        eprintln!(
            "fetching {} ({} MB)",
            artefact.local,
            artefact.bytes / 1_048_576
        );
        download(&url, &path)
            .with_context(|| format!("downloading {} from {url}", artefact.local))?;

        verify(&path, artefact.sha256).with_context(|| {
            format!(
                "{} did not match its expected hash and has been discarded",
                artefact.local
            )
        })?;
    }

    eprintln!("model ready in {}", dir.display());
    Ok(())
}

/// Streams a URL to a file, writing to a temporary name first.
///
/// Writing in place would leave a half-finished file behind if the transfer
/// is interrupted, which the next run would have to distinguish from a real
/// one.
fn download(url: &str, destination: &Path) -> Result<()> {
    let partial = destination.with_extension("partial");
    let response = ureq::get(url).call().context("the request failed")?;

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(&partial)
        .with_context(|| format!("creating {}", partial.display()))?;
    std::io::copy(&mut reader, &mut file).context("the transfer was interrupted")?;
    file.sync_all().context("flushing the download to disk")?;
    drop(file);

    std::fs::rename(&partial, destination)
        .with_context(|| format!("moving the download into {}", destination.display()))?;
    Ok(())
}

/// Fails unless the file hashes to `expected`, deleting it if it does not.
fn verify(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256(path)?;
    if actual != expected {
        let _ = std::fs::remove_file(path);
        bail!("expected sha256 {expected}, got {actual}");
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {} to verify it", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(hex, "{byte:02x}")?;
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_artefact_pins_a_full_length_hash() {
        for artefact in ARTEFACTS {
            assert_eq!(
                artefact.sha256.len(),
                64,
                "{} must pin a full sha256",
                artefact.local
            );
            assert!(artefact.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(artefact.bytes > 0);
        }
    }

    #[test]
    fn verification_rejects_a_file_whose_contents_changed() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("weights");
        std::fs::write(&path, b"not the model").expect("writing");
        let error = verify(&path, ARTEFACTS[0].sha256).expect_err("must reject");
        assert!(format!("{error:#}").contains("expected sha256"));
        assert!(
            !path.exists(),
            "a file failing verification must not be left in place"
        );
    }

    #[test]
    fn hashing_matches_a_known_value() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("abc");
        std::fs::write(&path, b"abc").expect("writing");
        assert_eq!(
            sha256(&path).expect("hashing"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_reported_download_size_is_the_sum_of_the_artefacts() {
        assert_eq!(
            download_bytes(),
            ARTEFACTS.iter().map(|a| a.bytes).sum::<u64>()
        );
        assert!(download_bytes() > 100_000_000);
    }
}
