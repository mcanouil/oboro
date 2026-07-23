# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- feat: Write each workbook sheet to its own TSV file (`book.xlsx` with a sheet `Clients` becomes `book.Clients.clean.tsv`), keeping the tabular structure openable in a spreadsheet tool instead of flattening the workbook into one markdown file; sheet names are sanitised for the filesystem, redacted like filenames when `redact_filenames` is on, and numbered apart when they collide.
- feat: Read `.csv` and `.tsv` files, passed through as plain text so the cleaned output stays a valid tabular file.
- feat: Name each output after its input's format, so `data.csv` becomes `data.clean.csv` and `data.tsv` becomes `data.clean.tsv` while documents keep `.clean.md`; `restore` needs no change since it rewrites placeholders in any text file.
- feat: Refuse any two inputs whose sanitised outputs would land on one file, including sheet outputs, case-folded names, and aliased spellings of one path, before the refused document's values are stored in the vault.
- feat: Match a denylist term against its exact case with `case_sensitive = true`, so a short name such as `Bell` is redacted without also redacting the ordinary word `bell`; terms still ignore case by default, and no regular expression is needed to make one case-sensitive.
- feat: Tidy text and markdown input before cleaning it, so trailing spaces, runs of blank lines and blank lines at either end of the file do not survive into the output; indentation is kept, since it carries markdown structure, and `.csv` and `.tsv` are passed through byte for byte.

## 0.3.0 (2026-07-22)

### Features

- feat: Publish prebuilt ner binaries (`x86_64-unknown-linux-gnu-ner`, `aarch64-unknown-linux-gnu-ner`, `aarch64-apple-darwin-ner`), installable with `install.sh --features ner`; the Linux ones need glibc 2.39+ since ONNX Runtime has no musl build.
- feat: Publish a ner Docker image under `-ner` suffixed tags (`ner`, `<version>-ner`, `main-ner`) with the recognition model baked in and hash-verified at image build, so untold names are found with no download and no network at run time.

## 0.2.0 (2026-07-22)

### Features

- feat: Accept a directory argument to `clean` and `review`, cleaning every supported file it holds; `--recursive` descends into subdirectories, unsupported files are skipped and counted, and `--output` mirrors the input tree.
- feat: Redact PII found in the input filename so it no longer leaks into the output name (`jean@example.com.txt` becomes `EMAIL_1.clean.md`), sharing placeholders with the document body; on by default and disabled with `redact_filenames = false` in `oboro.toml`.

## 0.1.0 (2026-07-22)

### Features

- feat: Replace sensitive values in a document with stable placeholders, so the same value always becomes the same placeholder within a vault.
- feat: Keep the mapping in a local vault encrypted with AES-256-GCM and indexed by a keyed hash, so the database alone reveals neither the values nor whether a guessed value is present.
- feat: Bind each placeholder's sequence into the encryption, and create the vault, key and write-ahead-log sidecars owner-only, so a swapped row is detected and the files stay readable only by you.
- feat: Clean a document to placeholders with `clean`, and put the real values back into a model's answer with `restore`, both reading and writing standard input and output.
- feat: Step through every detection with `review`, a terminal screen for accepting or rejecting each one before anything is written.
- feat: Inspect and wipe the mapping with `map list` and `map purge`, and report the vault, configuration, supported formats and network use with `doctor`.
- feat: Detect emails, phone numbers, IBANs, payment cards, SIREN, SIRET, IP addresses and French addresses, each confirmed by a checksum or parser rather than a pattern alone.
- feat: Find names, organisations and addresses with a local multilingual recognition model, built with `--features ner` and fetched by `models pull`, which verifies downloads against pinned hashes.
- feat: Configure an allowlist, a denylist and custom identifier patterns through `oboro.toml`, with accented case folded so an entry such as `Société Générale` matches `SOCIÉTÉ GÉNÉRALE`.
- feat: Read `.txt`, `.md`, `.docx` including its headers, footers, footnotes and comments, `.xlsx`, and text-based `.pdf`, plus images through Tesseract when built with `--features ocr`.
- feat: Refuse a PDF whose pages yield almost no text, rather than producing output that looks sanitised but was never read.
- feat: Publish a Docker image, a single static binary on `distroless/static` with no shell and no network capability, and read the vault and key paths from `OBORO_VAULT` and `OBORO_KEY_FILE` so a container can point them at a mounted volume.
- feat: Install with a script that downloads the prebuilt binary and verifies it against the release checksums, or with prebuilt binaries that carry build provenance you can check with `gh attestation verify`.
