# oboro

An anonymisation layer between your files and a language model.

`oboro` replaces sensitive values in a document with stable placeholders, so
the text can be pasted into Claude Code, Copilot, Codex or Cursor without
leaking phone numbers, bank details, addresses or client names.
The mapping is kept in a local encrypted vault, so the model's answer can be
turned back into the real thing afterwards.

Nothing is ever sent anywhere: the tool is a single binary that makes no
network requests.

## How it works

```text
contract.txt ──► oboro clean ──► contract.clean.md ──► paste into a model
                     │                                        │
                     ▼                                        ▼
              vault (encrypted)  ◄────── oboro restore ◄── model's answer
```

The same value always becomes the same placeholder within a vault, so a
model still sees that two documents mention the same client.

## Usage

```bash
# Anonymise a document.
oboro clean contract.txt

# Look at the result, then paste it into a model.
cat contract.clean.md

# Put the real values back into the answer you got.
oboro restore answer.md

# See what the vault holds.
oboro map list

# Check the setup.
oboro doctor
```

Both `clean` and `restore` accept `--stdout`, so they compose in a pipeline:

```bash
oboro clean report.txt --stdout | pbcopy
```

### What it reads

| Format | How |
| --- | --- |
| `.txt`, `.md` | Read directly |
| `.docx` | Text runs pulled from the document body |
| `.xlsx`, `.xlsm` | Every sheet flattened to tab-separated rows |
| `.pdf` | Embedded text; scanned PDFs are refused, not half-read |
| `.png`, `.jpg`, `.tif` | Tesseract, with a build compiled `--features ocr` |

Optical character recognition is optional because it needs the Tesseract
system libraries. Without it the binary depends on nothing but Rust, and
images are refused with a message saying so rather than read as empty.

```bash
cargo build --release --features ocr
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

Personal and company names are matched from the denylist in `oboro.toml`.
Detecting them without being told is the job of the local NER model in a
later phase.

## Configuration

`oboro` reads the nearest `oboro.toml`, searching upwards from the working
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
| `~/.oboro/vault.db` | Placeholder mapping, values encrypted with AES-256-GCM |
| `~/.oboro/key` | The 32-byte key, created on first use |

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
- A PDF made of scanned images is refused rather than read. Export its pages
  as images and pass those to a build with OCR.
- Reading images needs the `ocr` feature and Tesseract; a plain build refuses
  them.
- Recognition accuracy on real photographs is not covered by an automated
  test yet.
- Older `.doc`, `.xls` and `.pptx` are not read at all.
- Detection favours redacting too much over too little. Use the allowlist
  when it goes too far.
- **Read the sanitised output before you share it.** No tool of this kind
  catches everything.

## Development

Build in the devcontainer. It carries the pinned Rust toolchain, Tesseract
and the OCR libraries the converter phases need, so the only thing your
machine needs is Docker.

In Visual Studio Code, reopen the folder in the container when prompted.
Otherwise use the image directly:

```bash
docker build -f .devcontainer/Dockerfile -t oboro-dev .devcontainer
docker run --rm -it -v "$PWD":/work -w /work -u vscode oboro-dev bash
```

Then, inside the container:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

The toolchain is pinned by `rust-toolchain.toml`, so the container, CI and a
host build all use the same compiler.

The test that matters most is `tests/leak.rs`: it plants known values in a
fixture and fails if any of them survives `clean`.

## Licence

MIT
