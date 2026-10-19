FROM mcr.microsoft.com/devcontainers/rust:1-1-bookworm

# System libraries the later phases need: Tesseract for OCR of images and
# scanned PDFs, and the clang and pkg-config toolchain the -sys crates want.
# Installed here so no contributor has to set up a host toolchain.
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    clang \
    libclang-dev \
    libleptonica-dev \
    libssl-dev \
    libtesseract-dev \
    pkg-config \
    tesseract-ocr \
    tesseract-ocr-eng \
    tesseract-ocr-fra \
  && apt-get clean \
  && rm -rf /var/lib/apt/lists/*

# The toolchain comes from rust-toolchain.toml, the same file CI and the
# release images read, so the container builds with the pinned compiler
# rather than whatever the base image ships.
USER vscode
COPY rust-toolchain.toml /tmp/toolchain/
RUN cd /tmp/toolchain \
  && rustup toolchain install \
  && cargo install cargo-nextest --locked
USER root
