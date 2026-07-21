# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- feat: Store the mapping in a local vault encrypted with AES-256-GCM, indexed by keyed hash so the database alone reveals nothing (#2).
- feat: Add `clean` to replace sensitive values in `.txt` and `.md` files with stable placeholders (#2).
- feat: Add `restore` to put real values back into a model's answer (#2).
- feat: Add `map list` and `map purge` to inspect and wipe the placeholder mapping (#2).
- feat: Add `doctor` to report the vault, configuration and supported formats (#2).
- feat: Detect emails, phone numbers, IBANs, payment cards, SIREN, SIRET, IP addresses and French addresses, each confirmed by a checksum or parser where one exists (#2).
- feat: Support `oboro.toml` for an allowlist, a denylist and custom identifier patterns (#2).
- feat: Read `.docx`, `.xlsx` and text-based `.pdf` documents, not just `.txt` and `.md`.
- feat: Read images through Tesseract when built with `--features ocr`, which `doctor` reports on.
- feat: Refuse a PDF that yields almost no text for its page count, rather than producing output that looks sanitised but was never read.
- feat: Find names, organisations and addresses with a local recognition model, built with `--features ner`, so they no longer have to be listed by hand (#6).
- feat: Add `models pull` and `models status` to fetch and inspect that model, verifying downloads against pinned hashes (#6).
- feat: Add `review`, a terminal screen for accepting or rejecting each detection before anything is written (#7).
- feat: Publish a Docker image, one static binary on `distroless/static` with no shell and no network capability, so the tool can be tried without a Rust toolchain (#10).
- feat: Read the vault and key paths from `OBORO_VAULT` and `OBORO_KEY_FILE`, so a container can point them at a mounted volume without repeating the flags (#10).
- feat: Add an install script that downloads the prebuilt binary and verifies it against the release checksums: `curl -fsSL https://m.canouil.dev/oboro/install.sh | bash`.

### Bug Fixes

- fix: Create the vault key file with owner-only permissions from the outset, rather than restricting them after writing.
- fix: Read Word headers, footers, footnotes and comments, not only the document body, so a letterhead is no longer silently dropped (#5).
- fix: Stop refusing short but genuine PDFs; only a page yielding essentially nothing is treated as scanned (#5).
- fix: Fold accented case in the allowlist, so an entry such as `Société Générale` matches `SOCIÉTÉ GÉNÉRALE` instead of being silently ignored.
- fix: Build the devcontainer against a pinned current toolchain, so the project compiles inside it.
- fix: Open a vault in a directory the tool did not create, instead of failing when it cannot change that directory's permissions. This made the vault unusable on a mounted volume.
- fix: Report `network: never contacted` in `doctor` for builds without the recognition model, which have no command that can open a socket.
