# Oboro

An anonymisation layer between your files and a language model.

`oboro` replaces sensitive values in a document with stable placeholders.
You can then paste the text into Claude Code, Copilot, Codex or Cursor.
No phone numbers, bank details, addresses or client names leak out.
Oboro keeps the mapping in a local, encrypted vault.
Use the vault to turn the model's answer back into the real values.

The tool never sends anything anywhere.
It is a single binary and makes no network requests.

## How it works

```text
contract.txt ──► oboro clean ──► contract.clean.md ──► paste into a model
                     │                                        │
                     ▼                                        ▼
              vault (encrypted)  ◄────── oboro restore ◄── model's answer
```

The same value always becomes the same placeholder within one vault.
So the model can still see that two documents mention the same client.

## Install

There are several ways to install Oboro, quickest first.
See the [Quickstart](https://m.canouil.dev/oboro/quickstart.html) for full detail.

**Install script** — installs the prebuilt binary for macOS or Linux, and verifies it against the release checksums:

```bash
curl -fsSL https://m.canouil.dev/oboro/install.sh | bash
```

Add `--features ner` to also detect names not on any list (Linux needs glibc 2.39+). Then fetch the model:

```bash
curl -fsSL https://m.canouil.dev/oboro/install.sh | bash -s -- --features ner
oboro models pull   # about 348 MB, once
```

**Windows** — installs the prebuilt binary and verifies it against the release checksums:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://m.canouil.dev/oboro/install.ps1 | iex"
```

This installs Oboro into `%LOCALAPPDATA%\Programs\oboro\bin`. It adds that folder to your user `PATH` if the folder is missing. You do not need administrator rights. Windows has no prebuilt build yet for name recognition (`ner`) or image reading (`ocr`). For either feature, build from source: `cargo build --release --features ner` (or `ocr`).

**Docker** — needs no toolchain, just one static binary. The vault volume holds the mapping, so you must create it:

```bash
docker volume create oboro-vault
docker run --rm -v oboro-vault:/vault -v "$PWD":/work -w /work \
  --user "$(id -u):$(id -g)" ghcr.io/mcanouil/oboro:latest clean contract.docx
```

The `ghcr.io/mcanouil/oboro:ner` tag carries the ner build with the recognition model already inside. It finds names with no download and no network at run time.

**Prebuilt binary, by hand** — download the archive for your machine and `SHA256SUMS` from the [releases page](https://github.com/mcanouil/oboro/releases). Verify the archive, then put `oboro` on your `PATH`.

**With Rust** — `cargo install --git https://github.com/mcanouil/oboro`.

**From source** — the only way to get `ocr`, since no prebuilt binary or image carries it:

```bash
git clone https://github.com/mcanouil/oboro.git
cd oboro
cargo build --release                        # default build
cargo build --release --features "ner,ocr"   # names and image OCR
```

**Devcontainer** — for building or contributing with only Docker on the host. It carries the pinned toolchain, Tesseract and the OCR libraries. Reopen the folder in the container in Visual Studio Code or a GitHub Codespace. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

**Uninstalling** — `oboro uninstall` removes everything the tool wrote, including the vault. See the [reference](https://m.canouil.dev/oboro/reference.html#uninstall).

The default prebuilt binary and Docker image carry no optional feature. Name recognition (`ner`) links ONNX Runtime, which has no musl build. So its prebuilt forms are separate: glibc release archives from the install script, and the `:ner` image. Optical character recognition (`ocr`) needs the Tesseract shared libraries at run time, so `ocr` stays a source-only build.

## In Claude Code

Claude Code reads files itself. Pasting a cleaned copy into it protects
nothing, because the agent already read the original. Two hooks put Oboro
in that path. The plugin installs both hooks, plus the skill that explains
what they do:

```text
/plugin marketplace add mcanouil/oboro
/plugin install oboro@oboro
```

The plugin still needs the binary on your `PATH`. It cannot install one.
Until the binary is there, Oboro withholds every matching tool result and
refuses every matching write, rather than leave you unprotected.

If the binary is already installed, one command does the same job without a plugin:

```bash
oboro skill install --with-hooks
```

It asks whether to cover this project, in `.claude/settings.local.json` and
`.claude/skills/`, or every project, in `~/.claude/settings.json` and
`~/.claude/skills/`. `--project` and `--user` skip the question. `--dry-run`
prints what both halves would write, without writing either. Oboro plans
both halves before it writes either one. So if a scope refuses one half,
Oboro installs neither.

Drop `--with-hooks` to install only the skill. Use `oboro hook install` to
install only the hooks. Oboro never moves, reorders or removes anything
already in the settings. A hook that already names `oboro hook` is left
exactly as you wrote it. This is what the hooks half adds:

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

`post-tool-use` replaces a tool's result with a cleaned one. So the model
reads `[[PHONE_1]]` where the file held a phone number. `pre-tool-use` puts
the real values back into what the model writes. So the placeholder never
reaches your source. You need both hooks: with only `post-tool-use`, the
model writes placeholders straight into your files.

An agent that was never told what `[[PHONE_1]]` is will guess. It might
guess a bug in your file, a template to fill in, or a redaction to work
around. The skill explains what the placeholder is. It is the other half of
the command above, and it installs on its own too:

```bash
oboro skill install
```

`oboro skill show` prints the skill text without writing anything. The
plugin carries the same skill. Install the skill one way or the other, not
both.

For an agent other than Claude Code, install just the skill:

```bash
npx skills add mcanouil/oboro
```

That installs the explanation, not the machine. It installs no binary and
no hooks, and redacts nothing until you add them. It also symlinks the
skill by default. That is why `oboro skill install` afterwards refuses to
overwrite the path, and names it instead.

`oboro doctor` reports which halves are installed. It says whether they
came from the plugin or from your own settings, and whether the skill is
there. Believing the agent side is wired up is not the same as having done
it.

If Oboro cannot do its job, it does not fail silently. On the way out, it
withholds the result. On the way in, it refuses the call. Either way, it
tells you why.

Oboro never covers what you type yourself. The event that fires on a
prompt can add context to it, but cannot rewrite it. So a value you paste
into the chat reaches the model exactly as you typed it. Paste a document
instead, and the hook covers it. See
[Limitations](https://m.canouil.dev/oboro/limitations.html) for the full
account.

## Usage

Without an agent that has hooks, or when you handle a document yourself:

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

Both `clean` and `restore` accept `--stdout`. Both read standard input when
you pipe text in. So they compose in a pipeline, and nothing has to touch
disk first:

```bash
oboro clean report.txt --stdout | pbcopy
pbpaste | oboro restore
```

`clean` and `review` also accept a directory. They clean every supported
file inside it. Unsupported files are skipped and counted; they do not
stop the run:

```bash
# Every readable file in the folder, then its subfolders too.
oboro clean contracts/
oboro clean contracts/ --recursive --output sanitised/
```

With `--output`, Oboro mirrors the directory's subfolders under it. So
files that share a name in different subfolders do not collide.

### What it reads

| Format                 | How                                                               |
| ---------------------- | ----------------------------------------------------------------- |
| `.txt`, `.md`          | Read directly; trailing spaces and blank-line runs tidied         |
| `.csv`, `.tsv`         | Read byte for byte; the output keeps the tabular extension        |
| `.docx`                | Text runs from the body, headers, footers, footnotes and comments |
| `.pptx`                | Text from slides, speaker notes and comments                      |
| `.eml`                 | Headers, every body part, forwarded messages, attachment names    |
| `.odt`                 | Body, headers and footers, annotations, footnotes, image alt text |
| `.xlsx`, `.xlsm`       | One TSV file per sheet, named `book.<sheet>.clean.tsv`            |
| `.pdf`                 | Embedded text; a scan needs a build compiled `--features ocr`     |
| `.png`, `.jpg`, `.tif` | Tesseract, with a build compiled `--features ocr`                 |

Optical character recognition is optional, because it needs the Tesseract
system libraries. Without it, the binary depends on nothing but Rust, and
Oboro refuses images with a message, rather than reading them as empty.
With it, Oboro uses whatever trained data Tesseract has installed, so you
do not need to declare a language; `ocr_languages` picks among them.

```bash
cargo build --release --features ocr
```

### What gets detected

Detection does not depend on the document's language. A file that mixes
languages is handled in one pass. This build recognises:

| Kind                                  | How it is verified                                 |
| -------------------------------------- | ---------------------------------------------------- |
| Email addresses                       | Pattern                                            |
| Phone numbers                         | `libphonenumber`                                   |
| IBANs                                 | ISO 13616 mod-97 checksum                          |
| Payment cards                         | Luhn checksum, 13 to 19 digits                     |
| SIRET                                 | Luhn on both the SIREN prefix and the whole number |
| SIREN                                 | Luhn checksum                                      |
| IP addresses                          | Parsed as IPv4 or IPv6                             |
| Street addresses and postcodes        | Pattern                                            |
| Anything you list yourself            | Your regular expressions and terms                 |

Oboro matches street addresses in the three word orders languages use. So
`12 rue de la Paix`, `10 Downing Street` and `Hauptstraße 5` are all read,
with nothing declared. Two settings exist as hints, and neither is
required. `regions` widens which national phone formats are read; an
international `+` number is caught whatever it holds. `ocr_languages`
names the language an image is written in. See
[Limitations](https://m.canouil.dev/oboro/limitations.html#languages) for
what the postcode patterns cover and miss.

A local, multilingual recognition model finds personal and company names.
Any build with `--features ner` includes it. The install script and the
`:ner` Docker image both ship a prebuilt model. From source, build it
yourself:

```bash
cargo build --release --features ner   # downloads ONNX Runtime while building
oboro models pull   # about 348 MB, once, verified against pinned hashes
```

The build fetches ONNX Runtime. So `--features ner` needs network access
at build time, and a fully offline build fails there. The model itself
runs on your machine. Once built, `models pull` is the only command that
touches the network, and only when you run it. The `:ner` image skips
even that, since the model is baked in.

Without the model, names are matched from the denylist in `oboro.toml`
instead.

The model over-redacts, so `oboro review` exists to put some of it back.
It lists every detection with its kind, confidence and surrounding line.
You accept or reject each one before Oboro writes a single byte:

```text
j/k move   space toggle   a accept all   n reject none   w write   s skip   q quit
```

Rejecting a detection leaves the value in the output. Oboro never records
a rejected value in the vault.

**The model over-redacts, deliberately.** A real name and an ordinary
phrase score almost the same: "Thomas Bernard" scores 0.237 while "The
quick brown fox" scores 0.218. No threshold separates them cleanly. So the
default errs towards redacting, and expects you to read the result. Raise
`ner_threshold` to redact less, at the risk of missing names. Lower it to
redact more.

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

Oboro creates both files readable only by you: an owner-only file mode on
Unix, an ACL granting only your account on Windows. Oboro looks up values
through a keyed hash, not the plaintext. So the database alone reveals
neither the values nor whether a guessed value is present.

If you lose the key, no one can read the vault, including you. Pass
`--vault` and `--key` to keep a separate vault per project.

## Limitations

Read them before trusting the output with anything that matters.

- Identifiers that fail their own checksum are not recognised. A mistyped
  IBAN will not be detected.
- The recognition model redacts some ordinary prose as though it were a
  name. This is the intended direction of error, not a bug. Read the
  output before you send it.
- Without `--features ner`, names are only redacted if you list them.
- Oboro reads a PDF made of scanned images only with the `ocr` feature,
  and only when the page image is `DCTDecode`, `JPXDecode` or
  `CCITTFaxDecode` with nothing layered over it. For any other page, Oboro
  refuses the file and names the reason, rather than reading it half-way.
  This includes a page with no image at all.
- Oboro decides whether to recognise a page one page at a time. So a
  scanned page inside an otherwise textual PDF is still read. A page with
  a few words and no image is kept as it is, not refused.
- Reading images needs the `ocr` feature and Tesseract. A plain build
  refuses them.
- Recognition is tested on rendered text, not on real photographs. Treat
  text recovered from an image as less reliable than text read directly.
- Older `.doc`, `.xls` and `.ppt` are not read at all, nor are `.ods` and
  `.odp`.
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
</content>
