# Oboro

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

## Install

Several ways in, quickest first; the [Quickstart](https://m.canouil.dev/oboro/quickstart.html) has the detail.

**Install script** — the prebuilt binary for macOS or Linux, verified against the release checksums:

```bash
curl -fsSL https://m.canouil.dev/oboro/install.sh | bash
```

Add `--features ner` for the build that also finds untold names (Linux: glibc 2.39+), then fetch its model:

```bash
curl -fsSL https://m.canouil.dev/oboro/install.sh | bash -s -- --features ner
oboro models pull   # about 348 MB, once
```

**Docker** — no toolchain, one static binary; the vault volume holds the mapping, so it is not optional:

```bash
docker volume create oboro-vault
docker run --rm -v oboro-vault:/vault -v "$PWD":/work -w /work \
  --user "$(id -u):$(id -g)" ghcr.io/mcanouil/oboro:latest clean contract.docx
```

The `ghcr.io/mcanouil/oboro:ner` tag carries the ner build with the recognition model already inside, so untold names are found with no download and no network at run time.

**Prebuilt binary, by hand** — download the archive for your machine and `SHA256SUMS` from the [releases page](https://github.com/mcanouil/oboro/releases), verify, and put `oboro` on your `PATH`.

**With Rust** — `cargo install --git https://github.com/mcanouil/oboro`.

**From source** — required for `ocr`, which no prebuilt binary or image carries:

```bash
git clone https://github.com/mcanouil/oboro.git
cd oboro
cargo build --release                        # default build
cargo build --release --features "ner,ocr"   # names and image OCR
```

**Devcontainer** — for building or contributing with only Docker on the host; it carries the pinned toolchain, Tesseract and the OCR libraries. Reopen the folder in the container in Visual Studio Code or a GitHub Codespace; see [`CONTRIBUTING.md`](CONTRIBUTING.md).

The default prebuilt binary and Docker image carry no optional feature. Name recognition (`ner`) links ONNX Runtime, which has no musl build, so its prebuilt forms are separate: glibc release archives via the install script, and the `:ner` image. Optical character recognition (`ocr`) needs the Tesseract shared libraries at run time, so it stays a source build.

## In Claude Code

Claude Code reads files itself, so pasting a cleaned copy into it protects
nothing: the agent already read the original. Two hooks put Oboro in that path,
and the plugin names them both along with the skill that explains what they do:

```text
/plugin marketplace add mcanouil/oboro
/plugin install oboro@oboro
```

The plugin still needs the binary on your `PATH`; it cannot install one. Until
there is one, every matching tool result is withheld and every matching write is
refused, rather than left unprotected.

With the binary already installed, one command does the same without a plugin:

```bash
oboro skill install --with-hooks
```

It asks whether to cover this project, in `.claude/settings.local.json` and
`.claude/skills/`, or every project, in `~/.claude/settings.json` and
`~/.claude/skills/`. `--project` and `--user` skip the question, and `--dry-run`
prints what both halves would write without writing either. Both are planned
before either is written, so a scope that refuses one installs neither.

Drop `--with-hooks` for the skill on its own, or use `oboro hook install` for
the hooks on their own. Nothing already in the settings is moved, reordered or
removed, and a hook already naming `oboro hook` is left exactly as you wrote it.
This is what the hooks half adds:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Read|Grep|Bash|WebFetch",
        "hooks": [{ "type": "command", "command": "oboro hook post-tool-use" }]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [{ "type": "command", "command": "oboro hook pre-tool-use" }]
      }
    ]
  }
}
```

`post-tool-use` replaces a tool's result with a cleaned one, so the model reads
`[[PHONE_1]]` where the file said a phone number. `pre-tool-use` puts the values
back into what the model writes, so the placeholder never reaches your source.
Both are needed: the first without the second means the model writes
placeholders into your files.

An agent that was never told what `[[PHONE_1]]` is will guess: a bug in your
file, a template to fill in, or a redaction to work around. The skill that
explains it is the other half of the command above, and it installs on its own
too:

```bash
oboro skill install
```

`oboro skill show` prints the text without writing anything. The plugin carries
the same skill, so install it one way or the other rather than both.

For an agent other than Claude Code, the skill without any of the above:

```bash
npx skills add mcanouil/oboro
```

That installs the explanation and not the machine: no binary, no hooks, and
nothing redacted until you add them. It also symlinks the skill by default,
which is why `oboro skill install` afterwards refuses the path by name rather
than overwriting it.

`oboro doctor` reports which halves are installed, whether they came from the
plugin or from your own settings, and whether the skill is there, since
believing the agent side is wired up is not the same as having done it.

If Oboro cannot do its job the tool does not quietly get its way: on the way out
the result is withheld, on the way in the call is refused, and you are told why.

What you type yourself is never covered. The event that fires on a prompt can
add context to a prompt but cannot rewrite it, so a value you paste into the
chat reaches the model as you typed it. Paste a document instead and the hook
covers it. The [Limitations](https://m.canouil.dev/oboro/limitations.html) page
is the honest account.

## Usage

Without an agent that has hooks, or for a document you are handling yourself:

```bash
# Anonymise a document.
oboro clean contract.txt

# Look at the result, then paste it into a model.
cat contract.clean.md

# Put the real values back into the answer you got.
oboro restore answer.md

# See what the vault holds.
oboro map list

# Go through the detections yourself before anything is written.
oboro review contract.txt

# Check the setup.
oboro doctor
```

Both `clean` and `restore` accept `--stdout`, and both read standard input when
text is piped in, so they compose in a pipeline and nothing has to be written to
disk first:

```bash
oboro clean report.txt --stdout | pbcopy
pbpaste | oboro restore
```

`clean` and `review` also take a directory, cleaning every supported file it
holds; unsupported files are skipped and counted rather than stopping the run:

```bash
# Every readable file in the folder, then its subfolders too.
oboro clean contracts/
oboro clean contracts/ --recursive --output sanitised/
```

With `--output` a directory's subfolders are mirrored under it, so files sharing
a name in different subfolders do not collide.

### What it reads

| Format                 | How                                                               |
| ---------------------- | ----------------------------------------------------------------- |
| `.txt`, `.md`          | Read directly; trailing spaces and blank-line runs tidied         |
| `.csv`, `.tsv`         | Read byte for byte; the output keeps the tabular extension        |
| `.docx`                | Text runs from the body, headers, footers, footnotes and comments |
| `.xlsx`, `.xlsm`       | One TSV file per sheet, named `book.<sheet>.clean.tsv`            |
| `.pdf`                 | Embedded text; scanned PDFs are refused, not half-read            |
| `.png`, `.jpg`, `.tif` | Tesseract, with a build compiled `--features ocr`                 |

Optical character recognition is optional because it needs the Tesseract
system libraries. Without it the binary depends on nothing but Rust, and
images are refused with a message saying so rather than read as empty. With
it, whatever trained data Tesseract has installed is used, so no language
needs declaring; `ocr_languages` picks among them.

```bash
cargo build --release --features ocr
```

### What gets detected

Detection does not depend on the document's language, and a file that mixes languages is handled in one pass.
This build recognises:

| Kind                                  | How it is verified                                 |
| ------------------------------------- | -------------------------------------------------- |
| Email addresses                       | Pattern                                            |
| Phone numbers                         | `libphonenumber`                                   |
| IBANs                                 | ISO 13616 mod-97 checksum                          |
| Payment cards                         | Luhn checksum, 13 to 19 digits                     |
| SIRET                                 | Luhn on both the SIREN prefix and the whole number |
| SIREN                                 | Luhn checksum                                      |
| IP addresses                          | Parsed as IPv4 or IPv6                             |
| Street addresses and postcodes        | Pattern                                            |
| Anything you list yourself            | Your regular expressions and terms                 |

Street addresses are matched in the three word orders languages use, so `12 rue
de la Paix`, `10 Downing Street` and `Hauptstraße 5` are all read without
anything being declared. Two settings exist as hints and neither is required:
`regions` widens which national phone formats are read, an international `+`
number being caught whatever it holds, and `ocr_languages` names what an image
is written in. See [Limitations](https://m.canouil.dev/oboro/limitations.html#languages)
for what the postcode patterns do and do not cover.

Personal and company names are found by a local, multilingual recognition
model, in any build with `--features ner`. The install script and the `:ner`
Docker image both ship one prebuilt; from source it is:

```bash
cargo build --release --features ner   # downloads ONNX Runtime while building
oboro models pull   # about 348 MB, once, verified against pinned hashes
```

The build fetches ONNX Runtime, so `--features ner` needs network access at
build time and a fully offline build fails there. The model itself runs on
your machine. Once built, `models pull` is the only command that touches the
network, and only when you run it; the `:ner` image skips even that, since
the model is baked in.

Without the model, names are matched from the denylist in `oboro.toml`
instead.

Since the model over-redacts, `oboro review` exists to put some of it back.
It lists every detection with its kind, confidence and surrounding line, and
you accept or reject each one before a single byte is written:

```text
j/k move   space toggle   a accept all   n reject none   w write   s skip   q quit
```

Rejecting a detection leaves the value in the output and never records it in
the vault.

**The model over-redacts, deliberately.** A real name inside a document and
an ordinary phrase score almost the same: "Thomas Bernard" scores 0.237 while
"The quick brown fox" scores 0.218. No threshold separates them, so the
default errs towards redacting and expects you to read the result. Raise
`ner_threshold` to redact less and risk missing names, or lower it to redact
more.

## Configuration

`oboro` reads the nearest `oboro.toml`, searching upwards from the working
directory. Every section is optional.

```toml
# Regions whose national phone number formats are read. Optional: without it
# the region comes from the locale, and international + numbers always work.
regions = ["FR", "GB"]

# Languages requested from Tesseract when reading images. Optional: without it
# whatever trained data is installed is used.
ocr_languages = ["fra", "eng"]

# The local recognition model. Lower the threshold to redact more.
ner_enabled = true
ner_threshold = 0.15

# Redact PII in the input filename too, not just the contents.
redact_filenames = true

# Values that must never be redacted.
allowlist = ["My Own Company Ltd"]

# Terms that must always be redacted.
# Case is ignored unless case_sensitive = true.
[[denylist]]
term = "Acme Consulting SARL"
kind = "provider"

# Your own identifier formats.
[[patterns]]
name = "contract number"
regex = "CT-[0-9]{6}"
```

## Where your data lives

| Path                | Contents                                               |
| ------------------- | ------------------------------------------------------ |
| `~/.oboro/vault.db` | Placeholder mapping, values encrypted with AES-256-GCM |
| `~/.oboro/key`      | The 32-byte key, created on first use                  |

Both are created with owner-only permissions. Values are looked up through a
keyed hash rather than the plaintext, so the database on its own reveals
neither the values nor whether a guessed value is present.

Lose the key and the vault cannot be read, including by you. Pass `--vault`
and `--key` to keep a separate vault per project.

## Limitations

Read them before trusting the output with anything that matters.

- Identifiers that fail their own checksum are not recognised. A mistyped
  IBAN will not be detected.
- The recognition model redacts some ordinary prose as though it were a
  name. This is the intended direction of error, not a bug, but it means the
  output needs reading before you send it.
- Without `--features ner`, names are only redacted if you list them.
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

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for bug reporting, development setup, and commit conventions.

## Citation

If you use _Oboro_ in your work, please cite it.
Citation metadata is provided in [`CITATION.cff`](CITATION.cff).
GitHub renders it via the "Cite this repository" widget on the repository sidebar.

## License

This project is licensed under the MIT License.
See the [LICENSE](LICENSE) file for details.
