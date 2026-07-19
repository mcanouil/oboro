# hush

An anonymisation layer between your files and a language model.

`hush` replaces sensitive values in a document with stable placeholders, so
the text can be pasted into Claude Code, Copilot, Codex or Cursor without
leaking phone numbers, bank details, addresses or client names.
The mapping is kept in a local encrypted vault, so the model's answer can be
turned back into the real thing afterwards.

Nothing is ever sent anywhere: the tool is a single binary that makes no
network requests.

## How it works

```text
contract.txt ──► hush clean ──► contract.clean.md ──► paste into a model
                     │                                        │
                     ▼                                        ▼
              vault (encrypted)  ◄────── hush restore ◄── model's answer
```

The same value always becomes the same placeholder within a vault, so a
model still sees that two documents mention the same client.

## Usage

```bash
# Anonymise a document.
hush clean contract.txt

# Look at the result, then paste it into a model.
cat contract.clean.md

# Put the real values back into the answer you got.
hush restore answer.md

# See what the vault holds.
hush map list

# Check the setup.
hush doctor
```

Both `clean` and `restore` accept `--stdout`, so they compose in a pipeline:

```bash
hush clean report.txt --stdout | pbcopy
```

### What gets detected

This build recognises, in French and English documents:

| Kind | How it is verified |
| --- | --- |
| Email addresses | Pattern |
| Phone numbers | `libphonenumber` |
| IBANs | ISO 13616 mod-97 checksum |
| Payment cards | Luhn checksum, 13 to 19 digits |
| SIRET | Luhn on both the SIREN prefix and the whole number |
| SIREN | Luhn checksum |
| IP addresses | Parsed as IPv4 or IPv6 |
| French street addresses and postcodes | Pattern |
| Anything you list yourself | Your regular expressions and terms |

Personal and company names are matched from the denylist in `hush.toml`.
Detecting them without being told is the job of the local NER model in a
later phase.

## Configuration

`hush` reads the nearest `hush.toml`, searching upwards from the working
directory. Every section is optional.

```toml
# Region used to interpret national phone number formats.
default_region = "FR"

# Values that must never be redacted.
allowlist = ["My Own Company Ltd"]

# Terms that must always be redacted.
[[denylist]]
term = "Acme Consulting SARL"
kind = "provider"

# Your own identifier formats.
[[patterns]]
name = "contract number"
regex = "CT-[0-9]{6}"
```

## Where your data lives

| Path | Contents |
| --- | --- |
| `~/.hush/vault.db` | Placeholder mapping, values encrypted with AES-256-GCM |
| `~/.hush/key` | The 32-byte key, created on first use |

Both are created with owner-only permissions. Values are looked up through a
keyed hash rather than the plaintext, so the database on its own reveals
neither the values nor whether a guessed value is present.

Lose the key and the vault cannot be read, including by you. Pass `--vault`
and `--key` to keep a separate vault per project.

## Limitations

Read them before trusting the output with anything that matters.

- Identifiers that fail their own checksum are not recognised. A mistyped
  IBAN will not be detected.
- Names are only redacted if you list them, until the NER phase lands.
- Only `.txt` and `.md` are read so far. Office documents, PDFs and images
  come next.
- Detection favours redacting too much over too little. Use the allowlist
  when it goes too far.
- **Read the sanitised output before you share it.** No tool of this kind
  catches everything.

## Development

Open the repository in the devcontainer, which carries the Rust toolchain and
the OCR libraries the later phases need. Then:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

The test that matters most is `tests/leak.rs`: it plants known values in a
fixture and fails if any of them survives `clean`.

## Licence

MIT
