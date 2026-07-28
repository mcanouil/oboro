# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- feat: Read a scanned PDF rather than refusing it, on a build with `--features ocr`: the images on its pages go through the recogniser, covering both the way a scanner stores a colour page (`DCTDecode`) and the way it stores a bilevel one (`CCITTFaxDecode`), needing no library beyond the Tesseract the feature already asks for; a page whose image is in a codec that cannot be read is refused by name rather than half read, as is a page carrying no image at all. (#75)
- feat: Install both halves with one command, `oboro skill install --with-hooks`, which writes the skill and names both hooks in the same scope, asks once and shows both files it would write, and plans both before writing either, so a scope that refuses one installs neither; drop the flag for the skill alone, or use `oboro hook install` for the hooks alone. (#74)
- feat: Install the hooks and the skill in one step from a Claude Code plugin marketplace, with `/plugin marketplace add mcanouil/oboro` then `/plugin install oboro@oboro`; the plugin is this repository, so it ships the skill the binary carries rather than a copy of it, and its hooks go through a wrapper that withholds the result and refuses the call when no `oboro` is on `PATH`, rather than leaving a machine unprotected while looking installed. (#74)
- feat: Report an enabled Oboro plugin in `oboro doctor`, naming the settings file that enables it, and say of the hooks and the skill that the plugin carries them rather than that they are missing, since what it carries lives in its own files and reporting it as absent would send you to install what you already have. (#74)
- feat: Say so when `oboro hook install`, or `oboro skill install --with-hooks`, is about to name hooks a plugin already carries, since both copies would then run on every matching tool call. (#74)
- docs: Name `npx skills add mcanouil/oboro` as the skill-only path, for agents other than Claude Code, saying plainly that it installs the explanation and not the machine and that it symlinks the skill, which is why `oboro skill install` afterwards refuses the path rather than overwriting it. (#74)
- feat: Name both hooks in your agent's settings with `oboro hook install`, so wiring Oboro into Claude Code no longer means pasting JSON by hand; `--project` writes `.claude/settings.local.json` and `--user` writes `~/.claude/settings.json`, without either it asks which, and `--dry-run` prints the settings as they would end up.
- feat: Merge into the settings file rather than rewriting it: every other key keeps its place and its order, another tool's hooks on the same event are left where they are, and a hook already naming `oboro hook` is left exactly as you wrote it, matcher included, so a matcher you narrowed by hand survives an install.
- feat: Refuse rather than replace a settings file Oboro cannot merge into, naming it: invalid JSON, a root that is not an object, or a `hooks` entry of the wrong shape. Nothing is written through a symbolic link, and the file is replaced by renaming a complete one into place.
- feat: Say so when the hooks just installed name an `oboro` that is not on `PATH`, since a hook the agent cannot run fails closed on every matching tool call.
- feat: Tell an agent what the hooks have done to what it reads, with `oboro skill install`, which writes a skill explaining that `[[EMAIL_1]]` is a real value it cannot see rather than a bug or a template, and that writing the placeholder back verbatim is correct because `pre-tool-use` restores it; the text is compiled into the binary so it cannot drift from the hooks it describes, `oboro skill show` prints it without writing anything, and `oboro doctor` reports both scopes.
- feat: Ask which scope to install the skill into rather than guessing, since the wrong scope fails silently; `--project` and `--user` skip the question, and with no terminal to ask the command fails and names both flags.
- feat: Leave an edited skill where it is, writing what would have been installed to `SKILL.md.oboro-proposed` beside it so the edit can be compared rather than lost; `--force` overwrites instead, and nothing is written through a symbolic link.
- feat: Clean text piped in on standard input, with `printf '...' | oboro clean` or an explicit `-`, so a caller holding text in memory no longer has to write it to a temporary file first; the cleaned text goes to standard output, `--output` is refused since there is no name to write alongside, `-` cannot be combined with file paths, and input that is not valid UTF-8 is refused rather than mangled.
- feat: Report whether the agent hooks are installed in `oboro doctor`, naming the settings file, the tools each is matched against, and whether the program it names can be run; both events are reported even when neither is found, since having only the cleaning half means placeholders reach your files.
- feat: Put real values back into what an agent writes with `oboro hook pre-tool-use`, which answers a Claude Code `PreToolUse` payload and replaces the tool's arguments, so a placeholder the model echoed back becomes the value again before a `Write` or an `Edit` touches a file; every string in the arguments is restored whatever field it sits in, arguments holding no placeholder are left alone, a placeholder the vault never issued is reported and left in place, and a failure refuses the call rather than writing placeholders into your source.
- feat: Clean what an agent reads with `oboro hook post-tool-use`, which answers a Claude Code `PostToolUse` payload on standard input and replaces the tool's result with a cleaned one, so a `Read`, `Grep`, `Bash` or `WebFetch` result reaches the model as placeholders; a structured result keeps its shape with every string in it cleaned, and when cleaning fails the result is withheld rather than shown, with the reason reported to you and not to the model.
- feat: Restore an answer piped in on standard input, with `pbpaste | oboro restore` or an explicit `-`, so `restore` composes in a pipeline without a temporary file and without the `/dev/stdin` trick; piped text has no file to rewrite, so the restored text always goes to standard output.

### Bug Fixes

- fix: Allocate placeholders under one atomic step, so `oboro` invocations sharing a vault no longer fail with `database is locked` or a `UNIQUE constraint failed` error when two of them meet the same new value at the same moment; each value still maps to exactly one placeholder.
- fix: Stop quietly when a reader closes the output pipe, so `oboro clean note.txt --stdout | head -n 1` ends instead of reporting a crash; `map list` already did this, and now `clean` and `restore` do too.
- fix: Stop quietly in `oboro doctor | head -n 1` and `oboro models status | head -n 1` as well, the last two commands whose output was still written with macros that crash on a closed pipe.
- fix: Stop quietly when a reader closes the error pipe too, so `oboro clean notes/ 2>&1 | head -n 1` ends instead of reporting a crash, and a command that fails still exits 1 rather than 101; every progress and summary line, in `clean`, `restore`, `map`, `review` and `models pull`, now goes through one writer that drops a line it cannot deliver instead of panicking.

## 0.4.0 (2026-07-23)

### Features

- feat: Write each workbook sheet to its own TSV file (`book.xlsx` with a sheet `Clients` becomes `book.Clients.clean.tsv`), keeping the tabular structure openable in a spreadsheet tool instead of flattening the workbook into one markdown file; sheet names are sanitised for the filesystem, redacted like filenames when `redact_filenames` is on, and numbered apart when they collide.
- feat: Read `.csv` and `.tsv` files, passed through as plain text so the cleaned output stays a valid tabular file.
- feat: Name each output after its input's format, so `data.csv` becomes `data.clean.csv` and `data.tsv` becomes `data.clean.tsv` while documents keep `.clean.md`; `restore` needs no change since it rewrites placeholders in any text file.
- feat: Refuse any two inputs whose sanitised outputs would land on one file, including sheet outputs, case-folded names, and aliased spellings of one path, before the refused document's values are stored in the vault.
- feat: Match a denylist term against its exact case with `case_sensitive = true`, so a short name such as `Bell` is redacted without also redacting the ordinary word `bell`; terms still ignore case by default, and no regular expression is needed to make one case-sensitive.
- feat: Match street addresses in the three word orders languages write them in, so `10 Downing Street`, `Hauptstraße 5` and `12 Kerkstraat` are read alongside `12 rue de la Paix`, with no language declared anywhere; postcodes now cover the British, Canadian, Dutch and American formats as well as five-digit ones, while a bare four-digit postcode stays unmatched on purpose since it cannot be told apart from a year.
- feat: Replace `default_region` with `regions`, a list of region codes whose national phone number formats are read, so a document holding numbers from several countries is handled at once; a number valid in any listed region is redacted, an international `+` number is read whatever the list holds, and an unknown code is now refused by name instead of silently ignored. Without the key the region comes from the environment's locale, and `oboro doctor` reports which regions are in force and where they came from. A configuration still using `default_region` is refused, naming the unknown key.
- feat: Choose the languages Tesseract reads images in from `ocr_languages`, or from whatever trained data is installed when it is unset, replacing the hard-coded `fra+eng` that made French trained data a requirement for reading any image at all; asking for a language with no trained data now says so and lists what is installed.
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
