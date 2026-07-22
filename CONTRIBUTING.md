# Contributing to Oboro

Thank you for helping to improve Oboro.

## Reporting a bug

Open an issue on [GitHub](https://github.com/mcanouil/oboro/issues) and include:

- The command you ran and the input format (`.docx`, `.pdf`, an image, and so on).
- What happened, and what you expected instead.
- The output of `oboro doctor`, so the enabled features are known.

Never paste real sensitive values into an issue.
Oboro exists to keep exactly those values off other people's machines.
Reproduce the problem with invented data, as the fixtures in `testdata/` do.

## Development setup

Build in the devcontainer.
It carries the pinned Rust toolchain, Tesseract and the OCR libraries, so the only thing your machine needs is Docker.

In Visual Studio Code, reopen the folder in the container when prompted.
Otherwise use the image directly:

```bash
docker build -f .devcontainer/Dockerfile -t oboro-dev .devcontainer
docker run --rm -it -v "$PWD":/work -w /work -u vscode oboro-dev bash
```

The toolchain is pinned by `rust-toolchain.toml`, so the container, CI and a host build all use the same compiler.

Run the three checks before opening a pull request:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

Each feature flag compiles different code, so lint them too:

```bash
cargo clippy --all-targets --features ner -- -D warnings
cargo clippy --all-targets --features ocr -- -D warnings
```

The test that must stay green is `tests/leak.rs`.
It plants known values in fixtures and fails if any of them survives `clean`.

The full guide, covering the source layout and how to add a recogniser or a format, is at [Development](https://m.canouil.dev/oboro/development.html).

## Commit conventions

- Use [Conventional Commits](https://www.conventionalcommits.org): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, and so on.
- Write the subject in the imperative mood, 72 characters or fewer.
- Keep one logical change per commit.
- Branch off `main` and open a pull request; never push to `main` directly.
