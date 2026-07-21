## Install

### Quick install (script)

```bash
curl -fsSL https://m.canouil.dev/oboro/install.sh | bash

# or pin this exact release
curl -fsSL https://m.canouil.dev/oboro/install.sh | bash -s -- --version %%VERSION%%
```

The script picks the archive for your machine, verifies it against `SHA256SUMS`, and installs into `/usr/local/bin` when writable, otherwise `~/.local/bin`.
It needs `bash` and `curl`; on a minimal distribution such as Alpine, install them first with `apk add bash curl`.

### Docker

```bash
docker volume create oboro-vault
docker run --rm \
  -v oboro-vault:/vault \
  -v "$PWD":/work -w /work \
  --user "$(id -u):$(id -g)" \
  ghcr.io/mcanouil/oboro:%%VERSION%% clean contract.docx
```

The vault volume is not optional.
Without it the mapping between placeholders and real values disappears with the container, and the document can never be restored.

### A prebuilt binary

Pick the archive for your machine from the table below, then:

```bash
VERSION=%%VERSION%%
TARGET=x86_64-unknown-linux-musl   # or whichever row matches

curl -fsSLO "https://github.com/mcanouil/oboro/releases/download/${VERSION}/oboro-${VERSION}-${TARGET}.tar.gz"
curl -fsSLO "https://github.com/mcanouil/oboro/releases/download/${VERSION}/SHA256SUMS"

# Check it is what was published.
sha256sum --ignore-missing --check SHA256SUMS

tar -xzf "oboro-${VERSION}-${TARGET}.tar.gz"
install -m 0755 oboro /usr/local/bin/oboro
```

On macOS, `shasum -a 256 --ignore-missing --check SHA256SUMS` does the same job.

### With Rust already installed

```bash
cargo install --git https://github.com/mcanouil/oboro --tag %%VERSION%%
```

### From source, with the optional features

The published binaries are the default build.
They read `.txt`, `.md`, `.docx`, `.xlsx` and text-based `.pdf`, and find structured values and anything on your denylist.

They do **not** find names nobody told them about, and they do not read images.
Both need system libraries, so they are compiled in rather than shipped:

```bash
cargo build --release --features ner   # names and organisations, then: oboro models pull
cargo build --release --features ocr   # images and scanned pages, needs Tesseract
```

If names are not being redacted, this is almost certainly why.
`oboro doctor` reports what any build can do.

## Verify what you downloaded

Beyond the checksum, every archive carries build provenance, so you can confirm it came from this repository's workflow and not from somewhere else:

```bash
gh attestation verify "oboro-%%VERSION%%-x86_64-unknown-linux-musl.tar.gz" \
  --repo mcanouil/oboro
```

This tool checks the model it downloads against a pinned hash before using it.
It would be inconsistent to ask you to trust its own binaries on sight.

## Which archive is which

| Archive | For |
| --- | --- |
| `x86_64-unknown-linux-musl` | Linux on Intel or AMD. Statically linked, so any distribution, glibc version or Alpine. |
| `aarch64-unknown-linux-musl` | Linux on ARM, including most cloud instances. Statically linked. |
| `aarch64-apple-darwin` | macOS on Apple silicon. |
| `x86_64-apple-darwin` | macOS on Intel. |

There is no Windows build.
The code that creates the vault key readable only by you is Unix-specific, and shipping a build where that quietly does nothing would misrepresent what the tool guarantees.

## Documentation

<https://m.canouil.dev/oboro/>

---

## Changes
