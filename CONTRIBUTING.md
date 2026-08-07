# Contributing to Oboro

Thank you for helping to improve Oboro.

## Reporting a bug

Open an issue on [GitHub](https://github.com/mcanouil/oboro/issues) and include:

- The command you ran and the input format (`.docx`, `.pdf`, an image, and so on).
- What happened, and what you expected instead.
- The output of `oboro doctor`, so we know which features are enabled.

Never paste real sensitive values into an issue.
Oboro exists to keep exactly those values off other people's machines.
Reproduce the problem with invented data, as the fixtures in `testdata/` do.

## Development setup

Build in the devcontainer.
It carries the pinned Rust toolchain, Tesseract and the OCR libraries.
Your machine only needs Docker.

In Visual Studio Code, reopen the folder in the container when prompted.
Otherwise use the image directly:

```bash
docker build -f .devcontainer/Dockerfile -t oboro-dev .devcontainer
docker run --rm -it -v "$PWD":/work -w /work -u vscode oboro-dev bash
```

The toolchain is pinned by `rust-toolchain.toml`.
So the container, CI and a host build all use the same compiler.

Run these four checks before you open a pull request.
CI runs the same ones:

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

`cargo doc` is easy to forget.
It is the only check that fails on a doc comment that links from a public item to a private one.

Each feature flag compiles different code.
Lint and test them too:

```bash
cargo clippy --all-targets --features ner -- -D warnings
cargo test --all-targets --features ner
cargo clippy --all-targets --features ocr -- -D warnings
cargo test --all-targets --features ocr
```

The test that must stay green is `tests/leak.rs`.
It plants known values in fixtures and fails if any of them survives `clean`.

See [Development](https://m.canouil.dev/oboro/development.html) for the full guide: source layout, and how to add a recogniser or a format.

## Commit conventions

- Use [Conventional Commits](https://www.conventionalcommits.org): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, and so on.
- Write the subject in the imperative mood, 72 characters or fewer.
- Keep one logical change per commit.
- Branch off `main` and open a pull request; never push to `main` directly.
