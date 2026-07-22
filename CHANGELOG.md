# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

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
