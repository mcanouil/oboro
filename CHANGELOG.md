# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### New Features

- feat: Add `clean` to replace sensitive values in `.txt` and `.md` files with stable placeholders.
- feat: Add `restore` to put real values back into a model's answer.
- feat: Add `map list` and `map purge` to inspect and wipe the placeholder mapping.
- feat: Add `doctor` to report the vault, configuration and supported formats.
- feat: Detect emails, phone numbers, IBANs, payment cards, SIREN, SIRET, IP addresses and French addresses, each confirmed by a checksum or parser where one exists.
- feat: Support `hush.toml` for an allowlist, a denylist and custom identifier patterns.
- feat: Store the mapping in a local vault encrypted with AES-256-GCM, indexed by keyed hash so the database alone reveals nothing.
