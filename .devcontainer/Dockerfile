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

# The base image lags behind current stable, which is older than the
# rust-version this crate declares. Updating here means the container can
# actually build the project rather than failing on the edition.
USER vscode
RUN rustup update stable \
  && rustup default stable \
  && rustup component add clippy rustfmt \
  && cargo install cargo-nextest --locked
USER root
