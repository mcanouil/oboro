//! Optical character recognition for images.
//!
//! Compiled only with the `ocr` feature, which needs the Tesseract and
//! Leptonica system libraries.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

/// Languages used when the configuration names none and nothing is installed.
///
/// A last resort rather than an assumption: it is only reached when the trained
/// data cannot be found on disk, in which case Tesseract itself decides whether
/// this exists.
const FALLBACK_LANGUAGE: &str = "eng";

/// How many installed languages are loaded at once when none are configured.
///
/// Tesseract loses both speed and accuracy as languages pile up, and a package
/// such as Homebrew's `tesseract-lang` installs about a hundred, so past this
/// point one language is the better guess and the configuration is the way to
/// say otherwise.
const MAX_UNCONFIGURED_LANGUAGES: usize = 4;

/// Directories Tesseract looks in when `TESSDATA_PREFIX` says nothing.
const TESSDATA_CANDIDATES: [&str; 3] = [
    "/usr/share/tessdata",
    "/usr/local/share/tessdata",
    "/opt/homebrew/share/tessdata",
];

/// Trained data files that are not languages.
const NOT_LANGUAGES: [&str; 3] = ["osd", "equ", "snum"];

/// Reads the text in `path`, recognising `languages` when it is not empty.
///
/// An empty `languages` is the usual case: whatever trained data is installed
/// is used, so no language has to be declared to read a document.
pub fn image_to_text(path: &Path, languages: &[String]) -> Result<String> {
    if !path.is_file() {
        bail!("cannot read {}: no such file", path.display());
    }

    let installed = tessdata_dir().map(|dir| installed_languages(&dir));
    let requested = select_languages(languages, installed.as_deref())?;

    let mut engine = leptess::LepTess::new(None, &requested).map_err(|error| {
        anyhow!(
            "starting Tesseract with languages '{requested}' failed: {error}. \
             Install the trained data for them, such as the tesseract-ocr-<language> \
             packages, or set ocr_languages in oboro.toml to what you have."
        )
    })?;

    engine
        .set_image(path)
        .with_context(|| format!("loading {} as an image", path.display()))?;

    let text = engine
        .get_utf8_text()
        .with_context(|| format!("recognising text in {}", path.display()))?;

    if text.trim().is_empty() {
        bail!(
            "no text was recognised in {}. If it does contain writing, it may be too low \
             resolution to read; otherwise there is nothing here to anonymise.",
            path.display()
        );
    }
    Ok(text)
}

/// Chooses the language string to hand Tesseract.
///
/// `installed` is `None` when the trained data cannot be located, which is not
/// an error: Tesseract may still find it somewhere this does not look.
fn select_languages(requested: &[String], installed: Option<&[String]>) -> Result<String> {
    if !requested.is_empty() {
        if let Some(installed) = installed {
            let missing: Vec<&str> = requested
                .iter()
                .map(String::as_str)
                .filter(|language| !installed.iter().any(|found| found == language))
                .collect();
            if !missing.is_empty() {
                bail!(
                    "no trained data for {}; installed: {}. Set ocr_languages in \
                     oboro.toml to what is installed, or install the missing data.",
                    missing.join(", "),
                    if installed.is_empty() {
                        "none".to_owned()
                    } else {
                        installed.join(", ")
                    }
                );
            }
        }
        return Ok(requested.join("+"));
    }

    let Some(installed) = installed.filter(|installed| !installed.is_empty()) else {
        return Ok(FALLBACK_LANGUAGE.to_owned());
    };

    if installed.len() > MAX_UNCONFIGURED_LANGUAGES {
        let chosen = installed
            .iter()
            .find(|language| *language == FALLBACK_LANGUAGE)
            .unwrap_or(&installed[0]);
        return Ok(chosen.clone());
    }

    // English first when it is there, since a document in another language
    // still tends to carry English words, and the order is a priority.
    let mut chosen: Vec<&str> = installed.iter().map(String::as_str).collect();
    chosen.sort_unstable_by_key(|language| (*language != FALLBACK_LANGUAGE, *language));
    Ok(chosen.join("+"))
}

/// Finds the directory holding Tesseract's trained data.
fn tessdata_dir() -> Option<PathBuf> {
    if let Ok(prefix) = std::env::var("TESSDATA_PREFIX") {
        let prefix = PathBuf::from(prefix);
        // Tesseract 4 wanted the parent of `tessdata`, Tesseract 5 wants the
        // directory itself, and both spellings are in the wild.
        for candidate in [prefix.join("tessdata"), prefix] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    // Distributions version the path, so the versioned directories are read
    // rather than guessed.
    let versioned = std::fs::read_dir("/usr/share/tesseract-ocr")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("tessdata"));

    TESSDATA_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .chain(versioned)
        .find(|candidate| candidate.is_dir())
}

/// Lists the languages installed in `dir`, sorted.
///
/// Script models such as `Latin.traineddata` and the non-language helpers are
/// left out: they are not what a document is written in.
fn installed_languages(dir: &Path) -> Vec<String> {
    let mut languages: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "traineddata" {
                return None;
            }
            let stem = path.file_stem()?.to_str()?;
            let is_script = stem.starts_with(|c: char| c.is_uppercase());
            (!is_script && !NOT_LANGUAGES.contains(&stem)).then(|| stem.to_owned())
        })
        .collect();
    languages.sort_unstable();
    languages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(languages: &[&str]) -> Vec<String> {
        languages.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_missing_image_is_reported_clearly() {
        let error =
            image_to_text(Path::new("/nonexistent/scan.png"), &[]).expect_err("must reject");
        assert!(format!("{error:#}").contains("no such file"));
    }

    #[test]
    fn a_file_that_is_not_an_image_is_rejected() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let path = dir.path().join("fake.png");
        std::fs::write(&path, "definitely not a png").expect("writing");
        assert!(image_to_text(&path, &[]).is_err());
    }

    #[test]
    fn configured_languages_are_used_in_the_order_given() {
        let chosen = select_languages(
            &owned(&["deu", "fra"]),
            Some(&owned(&["deu", "eng", "fra"])),
        )
        .expect("all installed");
        assert_eq!(chosen, "deu+fra");
    }

    #[test]
    fn a_configured_language_that_is_not_installed_is_reported() {
        let error = select_languages(&owned(&["zzz"]), Some(&owned(&["eng", "fra"])))
            .expect_err("must reject");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("zzz"), "unhelpful error: {rendered}");
        assert!(
            rendered.contains("eng, fra"),
            "the error must list what is installed: {rendered}"
        );
    }

    #[test]
    fn configured_languages_are_trusted_when_nothing_can_be_listed() {
        let chosen = select_languages(&owned(&["fra"]), None).expect("must not guess");
        assert_eq!(chosen, "fra");
    }

    #[test]
    fn without_configuration_a_few_installed_languages_are_all_used() {
        let chosen =
            select_languages(&[], Some(&owned(&["fra", "deu", "eng"]))).expect("must choose");
        assert_eq!(
            chosen, "eng+deu+fra",
            "English leads, then the rest alphabetically"
        );
    }

    #[test]
    fn without_configuration_one_installed_language_is_used_whatever_it_is() {
        let chosen = select_languages(&[], Some(&owned(&["fra"]))).expect("must choose");
        assert_eq!(chosen, "fra", "no language is assumed to be installed");
    }

    #[test]
    fn without_configuration_many_installed_languages_narrow_to_one() {
        let many = owned(&["ara", "deu", "eng", "fra", "ita", "spa"]);
        assert_eq!(
            select_languages(&[], Some(&many)).expect("must choose"),
            "eng"
        );

        let many_without_english = owned(&["ara", "deu", "fra", "ita", "spa"]);
        assert_eq!(
            select_languages(&[], Some(&many_without_english)).expect("must choose"),
            "ara"
        );
    }

    #[test]
    fn nothing_installed_falls_back_rather_than_failing_early() {
        assert_eq!(
            select_languages(&[], Some(&[])).expect("must choose"),
            FALLBACK_LANGUAGE
        );
        assert_eq!(
            select_languages(&[], None).expect("must choose"),
            FALLBACK_LANGUAGE
        );
    }

    #[test]
    fn script_models_and_helpers_are_not_languages() {
        let dir = tempfile::tempdir().expect("temporary directory");
        for name in [
            "eng.traineddata",
            "fra.traineddata",
            "Latin.traineddata",
            "osd.traineddata",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), "x").expect("writing");
        }
        assert_eq!(installed_languages(dir.path()), ["eng", "fra"]);
    }
}
