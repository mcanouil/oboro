# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Breaking Changes

- The project is renamed from `hush` to `oboro` (朧, Japanese for hazy or veiled by mist).
  The binary, the crate, the configuration file (`oboro.toml`) and the vault location (`~/.oboro/`) all change with it.
- The key derivation contexts changed with the name, so a vault written by a `hush` build cannot be read by `oboro`.
  Anything cleaned with a `hush` build cannot be restored; clean those documents again.
  Nothing shipped under the old name, so no migration path is provided.

### Bug Fixes

- fix: Fold accented case in the allowlist, so an entry such as `Société Générale` matches `SOCIÉTÉ GÉNÉRALE` instead of being silently ignored.
- fix: Create the vault key file with owner-only permissions from the outset, rather than restricting them after writing.
- fix: Build the devcontainer against a pinned current toolchain, so the project compiles inside it.

### New Features

- feat: Add `clean` to replace sensitive values in `.txt` and `.md` files with stable placeholders.
- feat: Add `restore` to put real values back into a model's answer.
- feat: Add `map list` and `map purge` to inspect and wipe the placeholder mapping.
- feat: Add `doctor` to report the vault, configuration and supported formats.
- feat: Detect emails, phone numbers, IBANs, payment cards, SIREN, SIRET, IP addresses and French addresses, each confirmed by a checksum or parser where one exists.
- feat: Support `oboro.toml` for an allowlist, a denylist and custom identifier patterns.
- feat: Store the mapping in a local vault encrypted with AES-256-GCM, indexed by keyed hash so the database alone reveals nothing.
